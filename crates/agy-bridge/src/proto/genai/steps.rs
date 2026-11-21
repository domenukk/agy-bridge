//! Trajectory step, legacy media, and user/model step messages (`genai`, `steps.proto`).

use super::{
    Content, ContentList, ElicitationCallStep, ElicitationResultStep, MediaResolution,
    UserInputContent, audio_content, document_content, image_content,
    steps_tools::{ToolCallStep, ToolResultStep},
    text_content, video_content,
};

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StepList {
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub steps: Vec<Step>,
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase", default)]
pub struct StepMetadata {
    #[serde(
        alias = "content_status",
        with = "crate::proto::serde_helpers::ProtoEnumField::<step_metadata::ContentStatus>",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub content_status: i32,
}
/// Nested message and enum types in `StepMetadata`.
pub mod step_metadata {
    crate::proto::serde_helpers::define_proto_enum! {
        pub enum ContentStatus {
            #[default]
            Unspecified = 0 => "CONTENT_STATUS_UNSPECIFIED",
            FallbackMessage = 1 => "CONTENT_STATUS_FALLBACK_MESSAGE",
            BlockedContent = 2 => "CONTENT_STATUS_BLOCKED_CONTENT",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Step {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub description: String,
    #[serde(
        rename = "type",
        flatten,
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub r#type: Option<step::Type>,
}
/// Nested message and enum types in `Step`.
pub mod step {
    #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum Type {
        #[serde(alias = "user_input")]
        UserInput(super::UserInputStep),
        #[serde(alias = "model_output")]
        ModelOutput(super::ModelOutputStep),
        Thought(super::ThoughtStep),
        #[serde(alias = "tool_call")]
        ToolCall(Box<super::ToolCallStep>),
        #[serde(alias = "tool_result")]
        ToolResult(Box<super::ToolResultStep>),
        Text(super::LegacyTextContent),
        Image(super::LegacyImageContent),
        Audio(super::LegacyAudioContent),
        Document(super::LegacyDocumentContent),
        Video(super::LegacyVideoContent),
        Compaction(super::CompactionStep),
        Content(super::ContentStep),
        #[serde(alias = "elicitation_call")]
        ElicitationCall(super::ElicitationCallStep),
        #[serde(alias = "elicitation_result")]
        ElicitationResult(super::ElicitationResultStep),
    }
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ContentStep {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub role: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub content: Vec<Content>,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ThoughtStep {
    #[serde(
        with = "crate::proto::serde_helpers::base64_bytes",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub signature: Vec<u8>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub summary: Vec<Content>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CompactionStep {
    #[serde(
        with = "crate::proto::serde_helpers::base64_bytes",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub signature: Vec<u8>,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LegacyTextContent {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub text: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub annotations: Vec<text_content::Annotation>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LegacyImageContent {
    #[serde(
        alias = "mime_type",
        with = "crate::proto::serde_helpers::ProtoEnumField::<image_content::MimeType>",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub mime_type: i32,
    #[serde(
        alias = "mime_type_string",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub mime_type_string: String,
    #[serde(
        with = "crate::proto::serde_helpers::ProtoEnumField::<MediaResolution>",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub resolution: i32,
    #[serde(
        alias = "data_or_uri",
        flatten,
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub data_or_uri: Option<image_content::DataOrUri>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LegacyAudioContent {
    #[serde(
        alias = "mime_type",
        with = "crate::proto::serde_helpers::ProtoEnumField::<audio_content::MimeType>",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub mime_type: i32,
    #[serde(
        alias = "mime_type_string",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub mime_type_string: String,
    #[serde(
        deserialize_with = "crate::proto::serde_helpers::deserialize_i32",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub rate: i32,
    #[serde(
        deserialize_with = "crate::proto::serde_helpers::deserialize_i32",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub channels: i32,
    #[serde(
        alias = "sample_rate",
        deserialize_with = "crate::proto::serde_helpers::deserialize_i32",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub sample_rate: i32,
    #[serde(
        alias = "data_or_uri",
        flatten,
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub data_or_uri: Option<audio_content::DataOrUri>,
    #[serde(
        alias = "processing_oneof",
        flatten,
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub processing_oneof: Option<audio_content::ProcessingOneof>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LegacyDocumentContent {
    #[serde(
        alias = "mime_type",
        with = "crate::proto::serde_helpers::ProtoEnumField::<document_content::MimeType>",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub mime_type: i32,
    #[serde(
        alias = "mime_type_string",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub mime_type_string: String,
    #[serde(
        with = "crate::proto::serde_helpers::ProtoEnumField::<MediaResolution>",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub resolution: i32,
    #[serde(
        alias = "data_or_uri",
        flatten,
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub data_or_uri: Option<document_content::DataOrUri>,
    #[serde(
        alias = "processing_oneof",
        flatten,
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub processing_oneof: Option<document_content::ProcessingOneof>,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LegacyVideoContent {
    #[serde(
        alias = "mime_type",
        with = "crate::proto::serde_helpers::ProtoEnumField::<video_content::MimeType>",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub mime_type: i32,
    #[serde(
        alias = "mime_type_string",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub mime_type_string: String,
    #[serde(
        with = "crate::proto::serde_helpers::ProtoEnumField::<MediaResolution>",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub resolution: i32,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub name: String,
    #[serde(
        alias = "data_or_uri",
        flatten,
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub data_or_uri: Option<video_content::DataOrUri>,
    #[serde(
        alias = "processing_oneof",
        flatten,
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub processing_oneof: Option<video_content::ProcessingOneof>,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UserInputStep {
    #[serde(
        alias = "input_content",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub input_content: Vec<UserInputContent>,
    #[serde(flatten, skip_serializing_if = "Option::is_none", default)]
    pub content: Option<user_input_step::Content>,
}
/// Nested message and enum types in `UserInputStep`.
pub mod user_input_step {
    #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum Content {
        #[serde(alias = "content_list")]
        ContentList(super::ContentList),
        #[serde(alias = "content_string")]
        ContentString(String),
    }
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ModelOutputStep {
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub content: Vec<Content>,
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase", default)]
pub struct TranscriptionStep {
    #[serde(
        alias = "source_index",
        deserialize_with = "crate::proto::serde_helpers::deserialize_i32",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub source_index: i32,
}

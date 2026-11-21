//! Elicitation and user input step messages (`genai`, `elicitation.proto` & `user_steps.proto`).

use super::{
    AudioContent, Content, ContentList, DocumentContent, ImageContent, TextContent, VideoContent,
};

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase", default)]
pub struct UserTimeout {}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ElicitationPreamble {
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub content: Vec<Content>,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ElicitationChoice {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub label: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub display: Vec<Content>,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ConfirmationRequest {
    #[serde(
        alias = "positive_choice",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub positive_choice: Option<ElicitationChoice>,
    #[serde(
        alias = "negative_choice",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub negative_choice: Option<ElicitationChoice>,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MultipleChoiceRequest {
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub choices: Vec<ElicitationChoice>,
    #[serde(
        alias = "allow_multiple_selections",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub allow_multiple_selections: bool,
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase", default)]
pub struct ConfirmationResponse {
    #[serde(
        alias = "is_confirmed",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub is_confirmed: bool,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MultipleChoiceResponse {
    #[serde(
        alias = "selected_choice_labels",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub selected_choice_labels: Vec<String>,
    #[serde(alias = "user_input", skip_serializing_if = "Option::is_none", default)]
    pub user_input: Option<ContentList>,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ElicitationCallStep {
    #[serde(
        alias = "elicitation_id",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub elicitation_id: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub preamble: Option<ElicitationPreamble>,
    #[serde(
        alias = "timeout_duration",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub timeout_duration: Option<crate::proto::google::protobuf::Duration>,
    #[serde(
        rename = "type",
        flatten,
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub r#type: Option<elicitation_call_step::Type>,
}
/// Nested message and enum types in `ElicitationCallStep`.
pub mod elicitation_call_step {
    #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum Type {
        #[serde(alias = "confirmation_request")]
        ConfirmationRequest(super::ConfirmationRequest),
        #[serde(alias = "multiple_choice_request")]
        MultipleChoiceRequest(super::MultipleChoiceRequest),
    }
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ElicitationResultStep {
    #[serde(
        alias = "elicitation_id",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub elicitation_id: String,
    #[serde(
        rename = "type",
        flatten,
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub r#type: Option<elicitation_result_step::Type>,
}
/// Nested message and enum types in `ElicitationResultStep`.
pub mod elicitation_result_step {
    #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum Type {
        #[serde(alias = "user_input")]
        UserInput(super::ContentList),
        #[serde(alias = "user_timeout")]
        UserTimeout(super::UserTimeout),
        Confirmation(super::ConfirmationResponse),
        #[serde(alias = "multiple_choice")]
        MultipleChoice(super::MultipleChoiceResponse),
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SlashCommand {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UserInputContent {
    #[serde(
        rename = "type",
        flatten,
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub r#type: Option<user_input_content::Type>,
}
/// Nested message and enum types in `UserInputContent`.
pub mod user_input_content {
    #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum Type {
        Text(super::TextContent),
        Image(super::ImageContent),
        Audio(super::AudioContent),
        Document(super::DocumentContent),
        Video(super::VideoContent),
        #[serde(alias = "slash_command")]
        SlashCommand(super::SlashCommand),
    }
}

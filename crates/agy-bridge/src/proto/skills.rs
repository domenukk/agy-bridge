//! Skill definitions (`genai.skills`).

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SkillFile {
    #[serde(alias = "mime_type", skip_serializing_if = "String::is_empty", default)]
    pub mime_type: String,
    #[serde(flatten, skip_serializing_if = "Option::is_none", default)]
    pub content: Option<skill_file::Content>,
}
/// Nested message and enum types in `SkillFile`.
pub mod skill_file {
    #[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum Content {
        Text(String),
        Data(#[serde(with = "crate::proto::serde_helpers::base64_bytes")] Vec<u8>),
    }
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Skill {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub name: String,
    #[serde(alias = "skill_id", skip_serializing_if = "String::is_empty", default)]
    pub skill_id: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub description: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub content: String,
    #[serde(
        alias = "allowed_tools",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub allowed_tools: Vec<String>,
    #[serde(skip_serializing_if = "std::collections::HashMap::is_empty", default)]
    pub metadata: std::collections::HashMap<String, String>,
    #[serde(flatten, skip_serializing_if = "Option::is_none", default)]
    pub resources: Option<skill::Resources>,
}
/// Nested message and enum types in `Skill`.
pub mod skill {
    #[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct InlineResources {
        #[serde(skip_serializing_if = "std::collections::HashMap::is_empty", default)]
        pub resources: std::collections::HashMap<String, super::SkillFile>,
    }
    #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum Resources {
        #[serde(alias = "inline_resources")]
        InlineResources(InlineResources),
        #[serde(alias = "zipped_resources")]
        ZippedResources(super::SkillFile),
    }
}

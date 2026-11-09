//! Generative AI content, tool call, and citation definitions (`genai`).

mod media;
pub use media::*;

/// Skill definitions (`genai.skills`).
pub mod skills {
    pub use crate::proto::skills::*;
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UrlCitation {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub url: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub title: String,
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FileCitation {
    #[serde(
        alias = "document_uri",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub document_uri: String,
    #[serde(alias = "file_name", skip_serializing_if = "String::is_empty", default)]
    pub file_name: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub source: String,
    #[serde(
        alias = "custom_metadata",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub custom_metadata: Option<Struct>,
    #[serde(
        alias = "page_number",
        deserialize_with = "crate::proto::serde_helpers::deserialize_i32",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub page_number: i32,
    #[serde(alias = "media_id", skip_serializing_if = "String::is_empty", default)]
    pub media_id: String,
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PlaceCitation {
    #[serde(alias = "place_id", skip_serializing_if = "String::is_empty", default)]
    pub place_id: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub url: String,
    #[serde(
        alias = "review_snippets",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub review_snippets: Vec<ReviewSnippet>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ReviewSnippet {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub title: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub url: String,
    #[serde(alias = "review_id", skip_serializing_if = "String::is_empty", default)]
    pub review_id: String,
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Struct {
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub fields: Vec<Field>,
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Field {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub value: Option<Value>,
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Value {
    #[serde(flatten, skip_serializing_if = "Option::is_none", default)]
    pub kind: Option<value::Kind>,
}
/// Nested message and enum types in `Value`.
pub mod value {
    #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum Kind {
        #[serde(rename = "nullValue", alias = "null_value")]
        Null(
            #[serde(
                with = "crate::proto::serde_helpers::ProtoEnumField::<crate::proto::google::protobuf::NullValue>"
            )]
            i32,
        ),
        #[serde(rename = "numberValue", alias = "number_value")]
        Number(#[serde(with = "crate::proto::serde_helpers::f64_num")] f64),
        #[serde(rename = "stringValue", alias = "string_value")]
        String(String),
        #[serde(rename = "boolValue", alias = "bool_value")]
        Bool(bool),
        #[serde(rename = "structValue", alias = "struct_value")]
        Struct(super::Struct),
        #[serde(rename = "listValue", alias = "list_value")]
        List(super::ListValue),
        #[serde(rename = "contentValue", alias = "content_value")]
        Content(super::Content),
    }
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ListValue {
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub values: Vec<Value>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GoogleSearch {
    #[serde(
        alias = "search_types",
        with = "crate::proto::serde_helpers::ProtoEnumVec::<google_search::SearchType>",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub search_types: Vec<i32>,
}
/// Nested message and enum types in `GoogleSearch`.
pub mod google_search {
    crate::proto::serde_helpers::define_proto_enum! {
        pub enum SearchType {
            #[default]
            Unspecified = 0 => "SEARCH_TYPE_UNSPECIFIED",
            WebSearch = 1 => "SEARCH_TYPE_WEB_SEARCH",
            ImageSearch = 2 => "SEARCH_TYPE_IMAGE_SEARCH",
            EnterpriseWebSearch = 3 => "SEARCH_TYPE_ENTERPRISE_WEB_SEARCH",
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ThoughtContent {
    #[serde(
        with = "crate::proto::serde_helpers::base64_bytes",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub signature: Vec<u8>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub summary: Vec<ThoughtSummaryContent>,
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ThoughtSummaryContent {
    #[serde(
        rename = "type",
        flatten,
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub r#type: Option<thought_summary_content::Type>,
}
/// Nested message and enum types in `ThoughtSummaryContent`.
pub mod thought_summary_content {
    #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum Type {
        Text(super::TextContent),
        Image(super::ImageContent),
    }
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Turn {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub role: String,
    #[serde(flatten, skip_serializing_if = "Option::is_none", default)]
    pub content: Option<turn::Content>,
}
/// Nested message and enum types in `Turn`.
pub mod turn {
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
pub struct TurnList {
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub turns: Vec<Turn>,
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolCallContent {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub id: String,
    #[serde(
        with = "crate::proto::serde_helpers::base64_bytes",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub signature: Vec<u8>,
    #[serde(
        rename = "type",
        flatten,
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub r#type: Option<tool_call_content::Type>,
}
/// Nested message and enum types in `ToolCallContent`.
pub mod tool_call_content {
    #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum Type {
        #[serde(alias = "function_call")]
        FunctionCall(super::FunctionCallContent),
        #[serde(rename = "codeExecutionCall", alias = "code_execution_call")]
        CodeExecution(super::CodeExecutionCallContent),
        #[serde(rename = "urlContextCall", alias = "url_context_call")]
        UrlContext(super::UrlContextCallContent),
        #[serde(rename = "mcpServerToolCall", alias = "mcp_server_tool_call")]
        McpServerTool(super::McpServerToolCallContent),
        #[serde(rename = "googleSearchCall", alias = "google_search_call")]
        GoogleSearch(super::GoogleSearchCallContent),
        #[serde(rename = "fileSearchCall", alias = "file_search_call")]
        FileSearch(super::FileSearchCallContent),
        #[serde(rename = "googleMapsCall", alias = "google_maps_call")]
        GoogleMaps(super::GoogleMapsCallContent),
    }
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolResultContent {
    #[serde(alias = "call_id", skip_serializing_if = "String::is_empty", default)]
    pub call_id: String,
    #[serde(
        with = "crate::proto::serde_helpers::base64_bytes",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub signature: Vec<u8>,
    #[serde(
        rename = "type",
        flatten,
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub r#type: Option<tool_result_content::Type>,
}
/// Nested message and enum types in `ToolResultContent`.
pub mod tool_result_content {
    #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum Type {
        #[serde(rename = "functionResult", alias = "function_result")]
        Function(super::FunctionResultContent),
        #[serde(rename = "codeExecutionResult", alias = "code_execution_result")]
        CodeExecution(super::CodeExecutionResultContent),
        #[serde(rename = "urlContextResult", alias = "url_context_result")]
        UrlContext(super::UrlContextResultContent),
        #[serde(rename = "googleSearchResult", alias = "google_search_result")]
        GoogleSearch(super::GoogleSearchResultContent),
        #[serde(rename = "mcpServerToolResult", alias = "mcp_server_tool_result")]
        McpServerTool(super::McpServerToolResultContent),
        #[serde(rename = "fileSearchResult", alias = "file_search_result")]
        FileSearch(super::FileSearchResultContent),
        #[serde(rename = "googleMapsResult", alias = "google_maps_result")]
        GoogleMaps(super::GoogleMapsResultContent),
    }
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FunctionCallContent {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub arguments: Option<Struct>,
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FunctionResultContent {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub name: String,
    #[serde(
        alias = "is_error",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub is_error: bool,
    #[serde(flatten, skip_serializing_if = "Option::is_none", default)]
    pub result: Option<function_result_content::Result>,
}
/// Nested message and enum types in `FunctionResultContent`.
pub mod function_result_content {
    #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum Result {
        #[serde(alias = "struct_result")]
        StructResult(super::Struct),
        #[serde(alias = "content_list")]
        ContentList(super::FunctionResultSubcontentList),
        #[serde(alias = "string_result")]
        StringResult(String),
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CodeExecutionCallContent {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub arguments: Option<code_execution_call_content::CodeExecutionCallArguments>,
}
/// Nested message and enum types in `CodeExecutionCallContent`.
pub mod code_execution_call_content {
    #[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct CodeExecutionCallArguments {
        #[serde(
            with = "crate::proto::serde_helpers::ProtoEnumField::<code_execution_call_arguments::Language>",
            skip_serializing_if = "crate::proto::serde_helpers::is_default",
            default
        )]
        pub language: i32,
        #[serde(skip_serializing_if = "String::is_empty", default)]
        pub code: String,
    }
    /// Nested message and enum types in `CodeExecutionCallArguments`.
    pub mod code_execution_call_arguments {
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
        #[repr(i32)]
        pub enum Language {
            #[default]
            Unspecified = 0,
            Python = 1,
            Bash = 2,
        }
        impl Language {
            /// String value of the enum field names used in the Protobuf definition.
            ///
            /// The values are not transformed in any way and thus are considered stable
            /// (if the Protobuf definition does not change) and safe for programmatic use.
            #[must_use]
            pub fn as_str_name(&self) -> &'static str {
                match self {
                    Self::Unspecified => "LANGUAGE_UNSPECIFIED",
                    Self::Python => "PYTHON",
                    Self::Bash => "BASH",
                }
            }
            /// Creates an enum from field names used in the Protobuf definition.
            #[must_use]
            pub fn from_str_name(value: &str) -> Option<Self> {
                match value {
                    "LANGUAGE_UNSPECIFIED" => Some(Self::Unspecified),
                    "PYTHON" => Some(Self::Python),
                    "BASH" => Some(Self::Bash),
                    _ => None,
                }
            }
        }
        impl TryFrom<i32> for Language {
            type Error = i32;
            fn try_from(value: i32) -> std::result::Result<Self, i32> {
                match value {
                    0 => Ok(Self::Unspecified),
                    1 => Ok(Self::Python),
                    2 => Ok(Self::Bash),
                    other => Err(other),
                }
            }
        }
        impl crate::proto::serde_helpers::ProtoEnum for Language {
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
        impl serde::Serialize for Language {
            fn serialize<S: serde::Serializer>(
                &self,
                serializer: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                crate::proto::serde_helpers::serialize_proto_enum(self, serializer)
            }
        }
        impl<'de> serde::Deserialize<'de> for Language {
            fn deserialize<D: serde::Deserializer<'de>>(
                deserializer: D,
            ) -> std::result::Result<Self, D::Error> {
                crate::proto::serde_helpers::deserialize_proto_enum(deserializer)
            }
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CodeExecutionResultContent {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub result: String,
    #[serde(
        alias = "is_error",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub is_error: bool,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UrlContextCallContent {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub arguments: Option<url_context_call_content::UrlContextCallArguments>,
}
/// Nested message and enum types in `UrlContextCallContent`.
pub mod url_context_call_content {
    #[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct UrlContextCallArguments {
        #[serde(skip_serializing_if = "Vec::is_empty", default)]
        pub urls: Vec<String>,
    }
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UrlContextResultContent {
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub result: Vec<url_context_result_content::UrlContextResult>,
    #[serde(
        alias = "is_error",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub is_error: bool,
}
/// Nested message and enum types in `UrlContextResultContent`.
pub mod url_context_result_content {
    #[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct UrlContextResult {
        #[serde(skip_serializing_if = "String::is_empty", default)]
        pub url: String,
        #[serde(
            with = "crate::proto::serde_helpers::ProtoEnumField::<url_context_result::Status>",
            skip_serializing_if = "crate::proto::serde_helpers::is_default",
            default
        )]
        pub status: i32,
    }
    /// Nested message and enum types in `UrlContextResult`.
    pub mod url_context_result {
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
        #[repr(i32)]
        pub enum Status {
            #[default]
            Unspecified = 0,
            Success = 1,
            Error = 2,
            Paywall = 3,
            Unsafe = 4,
        }
        impl Status {
            /// String value of the enum field names used in the Protobuf definition.
            ///
            /// The values are not transformed in any way and thus are considered stable
            /// (if the Protobuf definition does not change) and safe for programmatic use.
            #[must_use]
            pub fn as_str_name(&self) -> &'static str {
                match self {
                    Self::Unspecified => "STATUS_UNSPECIFIED",
                    Self::Success => "SUCCESS",
                    Self::Error => "ERROR",
                    Self::Paywall => "PAYWALL",
                    Self::Unsafe => "UNSAFE",
                }
            }
            /// Creates an enum from field names used in the Protobuf definition.
            #[must_use]
            pub fn from_str_name(value: &str) -> Option<Self> {
                match value {
                    "STATUS_UNSPECIFIED" => Some(Self::Unspecified),
                    "SUCCESS" => Some(Self::Success),
                    "ERROR" => Some(Self::Error),
                    "PAYWALL" => Some(Self::Paywall),
                    "UNSAFE" => Some(Self::Unsafe),
                    _ => None,
                }
            }
        }
        impl TryFrom<i32> for Status {
            type Error = i32;
            fn try_from(value: i32) -> std::result::Result<Self, i32> {
                match value {
                    0 => Ok(Self::Unspecified),
                    1 => Ok(Self::Success),
                    2 => Ok(Self::Error),
                    3 => Ok(Self::Paywall),
                    4 => Ok(Self::Unsafe),
                    other => Err(other),
                }
            }
        }
        impl crate::proto::serde_helpers::ProtoEnum for Status {
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
        impl serde::Serialize for Status {
            fn serialize<S: serde::Serializer>(
                &self,
                serializer: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                crate::proto::serde_helpers::serialize_proto_enum(self, serializer)
            }
        }
        impl<'de> serde::Deserialize<'de> for Status {
            fn deserialize<D: serde::Deserializer<'de>>(
                deserializer: D,
            ) -> std::result::Result<Self, D::Error> {
                crate::proto::serde_helpers::deserialize_proto_enum(deserializer)
            }
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct McpServerToolCallContent {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub name: String,
    #[serde(
        alias = "server_name",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub server_name: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub arguments: Option<Struct>,
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct McpServerToolResultContent {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub name: String,
    #[serde(
        alias = "server_name",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub server_name: String,
    #[serde(flatten, skip_serializing_if = "Option::is_none", default)]
    pub result: Option<mcp_server_tool_result_content::Result>,
}
/// Nested message and enum types in `McpServerToolResultContent`.
pub mod mcp_server_tool_result_content {
    #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum Result {
        #[serde(alias = "struct_result")]
        StructResult(super::Struct),
        #[serde(alias = "content_list")]
        ContentList(super::FunctionResultSubcontentList),
        #[serde(alias = "string_result")]
        StringResult(String),
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GoogleSearchCallContent {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub arguments: Option<google_search_call_content::GoogleSearchCallArguments>,
    #[serde(
        alias = "search_type",
        with = "crate::proto::serde_helpers::ProtoEnumField::<google_search::SearchType>",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub search_type: i32,
}
/// Nested message and enum types in `GoogleSearchCallContent`.
pub mod google_search_call_content {
    #[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct GoogleSearchCallArguments {
        #[serde(skip_serializing_if = "Vec::is_empty", default)]
        pub queries: Vec<String>,
    }
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GoogleSearchResultContent {
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub result: Vec<google_search_result_content::GoogleSearchResult>,
    #[serde(
        alias = "is_error",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub is_error: bool,
}
/// Nested message and enum types in `GoogleSearchResultContent`.
pub mod google_search_result_content {
    #[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct GoogleSearchResult {
        #[serde(
            alias = "search_suggestions",
            skip_serializing_if = "String::is_empty",
            default
        )]
        pub search_suggestions: String,
    }
}
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase", default)]
pub struct FileSearchCallContent {}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FileSearchResultContent {
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub result: Vec<file_search_result_content::FileSearchResult>,
}
/// Nested message and enum types in `FileSearchResultContent`.
pub mod file_search_result_content {
    #[derive(
        Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
    )]
    #[serde(rename_all = "camelCase", default)]
    pub struct FileSearchResult {}
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GoogleMapsCallContent {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub arguments: Option<google_maps_call_content::GoogleMapsCallArguments>,
}
/// Nested message and enum types in `GoogleMapsCallContent`.
pub mod google_maps_call_content {
    #[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct GoogleMapsCallArguments {
        #[serde(skip_serializing_if = "Vec::is_empty", default)]
        pub queries: Vec<String>,
    }
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GoogleMapsResultContent {
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub result: Vec<google_maps_result_content::GoogleMapsResult>,
}
/// Nested message and enum types in `GoogleMapsResultContent`.
pub mod google_maps_result_content {
    #[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct GoogleMapsResult {
        #[serde(skip_serializing_if = "Vec::is_empty", default)]
        pub places: Vec<google_maps_result::Places>,
        #[serde(
            alias = "widget_context_token",
            skip_serializing_if = "String::is_empty",
            default
        )]
        pub widget_context_token: String,
    }
    /// Nested message and enum types in `GoogleMapsResult`.
    pub mod google_maps_result {
        #[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
        #[serde(rename_all = "camelCase", default)]
        pub struct Places {
            #[serde(alias = "place_id", skip_serializing_if = "String::is_empty", default)]
            pub place_id: String,
            #[serde(skip_serializing_if = "String::is_empty", default)]
            pub name: String,
            #[serde(skip_serializing_if = "String::is_empty", default)]
            pub url: String,
            #[serde(
                alias = "review_snippets",
                skip_serializing_if = "Vec::is_empty",
                default
            )]
            pub review_snippets: Vec<super::super::ReviewSnippet>,
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FunctionResultSubcontent {
    #[serde(
        rename = "type",
        flatten,
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub r#type: Option<function_result_subcontent::Type>,
}
/// Nested message and enum types in `FunctionResultSubcontent`.
pub mod function_result_subcontent {
    #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum Type {
        Text(super::TextContent),
        Image(super::ImageContent),
        Video(super::VideoContent),
    }
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FunctionResultSubcontentList {
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub contents: Vec<FunctionResultSubcontent>,
}
crate::proto::serde_helpers::define_proto_enum! {
    pub enum MediaResolution {
        #[default]
        Unspecified = 0 => "MEDIA_RESOLUTION_UNSPECIFIED",
        Low = 1 => "LOW",
        Medium = 2 => "MEDIUM",
        High = 3 => "HIGH",
        UltraHigh = 4 => "ULTRA_HIGH",
    }
}

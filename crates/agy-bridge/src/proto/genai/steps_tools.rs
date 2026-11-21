//! Tool call and tool result step messages (`genai`, `steps.proto`).

use super::{
    AgentToolCallStep, AgentToolResultStep, ContentList, ReviewSnippet, Struct, Value,
    google_search,
};

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolCallStep {
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
    pub r#type: Option<tool_call_step::Type>,
}
/// Nested message and enum types in `ToolCallStep`.
pub mod tool_call_step {
    #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum Type {
        #[serde(alias = "function_call")]
        FunctionCall(super::FunctionCallStep),
        #[serde(alias = "code_execution_call")]
        CodeExecutionCall(super::CodeExecutionCallStep),
        #[serde(alias = "url_context_call")]
        UrlContextCall(super::UrlContextCallStep),
        #[serde(alias = "mcp_server_tool_call")]
        McpServerToolCall(super::McpServerToolCallStep),
        #[serde(alias = "google_search_call")]
        GoogleSearchCall(super::GoogleSearchCallStep),
        #[serde(alias = "file_search_call")]
        FileSearchCall(super::FileSearchCallStep),
        #[serde(alias = "google_maps_call")]
        GoogleMapsCall(super::GoogleMapsCallStep),
        #[serde(alias = "find_file_call")]
        FindFileCall(super::FindFileCallStep),
        #[serde(alias = "list_directory_call")]
        ListDirectoryCall(super::ListDirectoryCallStep),
        #[serde(alias = "search_directory_call")]
        SearchDirectoryCall(super::SearchDirectoryCallStep),
        #[serde(alias = "view_file_call")]
        ViewFileCall(super::ViewFileCallStep),
        #[serde(alias = "edit_file_call")]
        EditFileCall(super::EditFileCallStep),
        #[serde(alias = "generate_image_call")]
        GenerateImageCall(super::GenerateImageCallStep),
        #[serde(alias = "processing_call")]
        ProcessingCall(super::ProcessingCallStep),
        #[serde(alias = "skill_lookup_call")]
        SkillLookupCall(super::SkillLookupCallStep),
        #[serde(alias = "experimental_tool_call")]
        ExperimentalToolCall(super::ExperimentalToolCallStep),
        #[serde(alias = "manage_task_call")]
        ManageTaskCall(super::ManageTaskCallStep),
        #[serde(alias = "finish_call")]
        FinishCall(super::FinishCallStep),
        #[serde(alias = "agent_tool_call")]
        AgentToolCall(super::AgentToolCallStep),
    }
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolResultStep {
    #[serde(alias = "call_id", skip_serializing_if = "String::is_empty", default)]
    pub call_id: String,
    #[serde(
        with = "crate::proto::serde_helpers::base64_bytes",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub signature: Vec<u8>,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub error: String,
    #[serde(
        rename = "type",
        flatten,
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub r#type: Option<tool_result_step::Type>,
}
/// Nested message and enum types in `ToolResultStep`.
pub mod tool_result_step {
    #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum Type {
        #[serde(alias = "function_result")]
        FunctionResult(super::FunctionResultStep),
        #[serde(alias = "code_execution_result")]
        CodeExecutionResult(super::CodeExecutionResultStep),
        #[serde(alias = "url_context_result")]
        UrlContextResult(super::UrlContextResultStep),
        #[serde(alias = "google_search_result")]
        GoogleSearchResult(super::GoogleSearchResultStep),
        #[serde(alias = "mcp_server_tool_result")]
        McpServerToolResult(super::McpServerToolResultStep),
        #[serde(alias = "file_search_result")]
        FileSearchResult(super::FileSearchResultStep),
        #[serde(alias = "google_maps_result")]
        GoogleMapsResult(super::GoogleMapsResultStep),
        #[serde(alias = "find_file_result")]
        FindFileResult(super::FindFileResultStep),
        #[serde(alias = "list_directory_result")]
        ListDirectoryResult(super::ListDirectoryResultStep),
        #[serde(alias = "search_directory_result")]
        SearchDirectoryResult(super::SearchDirectoryResultStep),
        #[serde(alias = "edit_file_result")]
        EditFileResult(super::EditFileResultStep),
        #[serde(alias = "generate_image_result")]
        GenerateImageResult(super::GenerateImageResultStep),
        #[serde(alias = "processing_result")]
        ProcessingResult(super::ProcessingResultStep),
        #[serde(alias = "view_file_result")]
        ViewFileResult(super::ViewFileResultStep),
        #[serde(alias = "skill_lookup_result")]
        SkillLookupResult(super::SkillLookupResultStep),
        #[serde(alias = "experimental_tool_result")]
        ExperimentalToolResult(super::ExperimentalToolResultStep),
        #[serde(alias = "manage_task_result")]
        ManageTaskResult(super::ManageTaskResultStep),
        #[serde(alias = "finish_result")]
        FinishResult(super::FinishResultStep),
        #[serde(alias = "agent_tool_result")]
        AgentToolResult(super::AgentToolResultStep),
    }
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FunctionCallStep {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub arguments: Option<Struct>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CodeExecutionCallStep {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub arguments: Option<code_execution_call_step::CodeExecutionCallStepArguments>,
    #[serde(
        alias = "working_directory",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub working_directory: String,
}
/// Nested message and enum types in `CodeExecutionCallStep`.
pub mod code_execution_call_step {
    #[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct CodeExecutionCallStepArguments {
        #[serde(
            with = "crate::proto::serde_helpers::ProtoEnumField::<code_execution_call_step_arguments::Language>",
            skip_serializing_if = "crate::proto::serde_helpers::is_default",
            default
        )]
        pub language: i32,
        #[serde(skip_serializing_if = "String::is_empty", default)]
        pub code: String,
    }
    /// Nested message and enum types in `CodeExecutionCallStepArguments`.
    pub mod code_execution_call_step_arguments {
        crate::proto::serde_helpers::define_proto_enum! {
            pub enum Language {
                #[default]
                Unspecified = 0 => "LANGUAGE_UNSPECIFIED",
                Python = 1 => "PYTHON",
                Bash = 2 => "BASH",
            }
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UrlContextCallStep {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub arguments: Option<url_context_call_step::UrlContextCallStepArguments>,
}
/// Nested message and enum types in `UrlContextCallStep`.
pub mod url_context_call_step {
    #[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct UrlContextCallStepArguments {
        #[serde(skip_serializing_if = "Vec::is_empty", default)]
        pub urls: Vec<String>,
    }
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct McpServerToolCallStep {
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

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GoogleSearchCallStep {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub arguments: Option<google_search_call_step::GoogleSearchCallStepArguments>,
    #[serde(
        alias = "search_type",
        with = "crate::proto::serde_helpers::ProtoEnumField::<google_search::SearchType>",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub search_type: i32,
}
/// Nested message and enum types in `GoogleSearchCallStep`.
pub mod google_search_call_step {
    #[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct GoogleSearchCallStepArguments {
        #[serde(skip_serializing_if = "Vec::is_empty", default)]
        pub queries: Vec<String>,
    }
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase", default)]
pub struct FileSearchCallStep {}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GoogleMapsCallStep {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub arguments: Option<google_maps_call_step::GoogleMapsCallStepArguments>,
}
/// Nested message and enum types in `GoogleMapsCallStep`.
pub mod google_maps_call_step {
    #[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct GoogleMapsCallStepArguments {
        #[serde(skip_serializing_if = "Vec::is_empty", default)]
        pub queries: Vec<String>,
    }
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase", default)]
pub struct ProcessingCallStep {}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase", default)]
pub struct ProcessingResultStep {}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FunctionResultStep {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub name: String,
    #[serde(
        alias = "is_error",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub is_error: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub result: Option<Value>,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CodeExecutionResultStep {
    #[serde(
        alias = "is_error",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub is_error: bool,
    #[serde(
        alias = "exit_code",
        deserialize_with = "crate::proto::serde_helpers::deserialize_i32",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub exit_code: i32,
    #[serde(flatten, skip_serializing_if = "Option::is_none", default)]
    pub output: Option<code_execution_result_step::Output>,
}
/// Nested message and enum types in `CodeExecutionResultStep`.
pub mod code_execution_result_step {
    #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum Output {
        Result(String),
        Content(super::ContentList),
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UrlContextResultStep {
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub result: Vec<url_context_result_step::UrlContextResultItem>,
    #[serde(
        alias = "is_error",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub is_error: bool,
}
/// Nested message and enum types in `UrlContextResultStep`.
pub mod url_context_result_step {
    #[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct UrlContextResultItem {
        #[serde(skip_serializing_if = "String::is_empty", default)]
        pub url: String,
        #[serde(
            with = "crate::proto::serde_helpers::ProtoEnumField::<url_context_result_item::Status>",
            skip_serializing_if = "crate::proto::serde_helpers::is_default",
            default
        )]
        pub status: i32,
    }
    /// Nested message and enum types in `UrlContextResultItem`.
    pub mod url_context_result_item {
        crate::proto::serde_helpers::define_proto_enum! {
            pub enum Status {
                #[default]
                Unspecified = 0 => "STATUS_UNSPECIFIED",
                Success = 1 => "SUCCESS",
                Error = 2 => "ERROR",
                Paywall = 3 => "PAYWALL",
                Unsafe = 4 => "UNSAFE",
            }
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GoogleSearchResultStep {
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub result: Vec<google_search_result_step::GoogleSearchResultItem>,
    #[serde(
        alias = "is_error",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub is_error: bool,
}
/// Nested message and enum types in `GoogleSearchResultStep`.
pub mod google_search_result_step {
    #[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct GoogleSearchResultItem {
        #[serde(
            alias = "search_suggestions",
            skip_serializing_if = "String::is_empty",
            default
        )]
        pub search_suggestions: String,
    }
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct McpServerToolResultStep {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub name: String,
    #[serde(
        alias = "server_name",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub server_name: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub result: Option<Value>,
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase", default)]
pub struct FileSearchResultStep {}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GoogleMapsResultStep {
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub result: Vec<google_maps_result_step::GoogleMapsResultItem>,
}
/// Nested message and enum types in `GoogleMapsResultStep`.
pub mod google_maps_result_step {
    #[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct GoogleMapsResultItem {
        #[serde(skip_serializing_if = "Vec::is_empty", default)]
        pub places: Vec<google_maps_result_item::GoogleMapsResultPlaces>,
        #[serde(
            alias = "widget_context_token",
            skip_serializing_if = "String::is_empty",
            default
        )]
        pub widget_context_token: String,
    }
    /// Nested message and enum types in `GoogleMapsResultItem`.
    pub mod google_maps_result_item {
        #[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
        #[serde(rename_all = "camelCase", default)]
        pub struct GoogleMapsResultPlaces {
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

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FindFileCallStep {
    #[serde(alias = "file_path", skip_serializing_if = "String::is_empty", default)]
    pub file_path: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub query: String,
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase", default)]
pub struct FindFileResultStep {}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ListDirectoryCallStep {
    #[serde(
        alias = "directory_path",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub directory_path: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ListDirectoryResultStep {
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub entries: Vec<list_directory_result_step::Entry>,
}
/// Nested message and enum types in `ListDirectoryResultStep`.
pub mod list_directory_result_step {
    #[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct Entry {
        #[serde(skip_serializing_if = "String::is_empty", default)]
        pub name: String,
        #[serde(flatten, skip_serializing_if = "Option::is_none", default)]
        pub info: Option<entry::Info>,
    }
    /// Nested message and enum types in `Entry`.
    pub mod entry {
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
pub struct SearchDirectoryCallStep {
    #[serde(
        alias = "directory_path",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub directory_path: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub query: String,
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase", default)]
pub struct SearchDirectoryResultStep {
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
pub struct ViewFileCallStep {
    #[serde(alias = "file_path", skip_serializing_if = "String::is_empty", default)]
    pub file_path: String,
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
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ViewFileResultStep {
    #[serde(
        alias = "error_message",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub error_message: String,
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
    #[serde(
        alias = "total_lines",
        deserialize_with = "crate::proto::serde_helpers::deserialize_i32",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub total_lines: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EditFileCallStep {
    #[serde(alias = "file_path", skip_serializing_if = "String::is_empty", default)]
    pub file_path: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EditFileResultStep {
    #[serde(alias = "diff_block", skip_serializing_if = "Vec::is_empty", default)]
    pub diff_block: Vec<edit_file_result_step::DiffBlock>,
}
/// Nested message and enum types in `EditFileResultStep`.
pub mod edit_file_result_step {
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
        crate::proto::serde_helpers::define_proto_enum! {
            pub enum LineAction {
                #[default]
                Unspecified = 0 => "LINE_ACTION_UNSPECIFIED",
                Insert = 1 => "LINE_ACTION_INSERT",
                Delete = 2 => "LINE_ACTION_DELETE",
                None = 3 => "LINE_ACTION_NONE",
            }
        }
    }
    #[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
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
pub struct GenerateImageCallStep {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub prompt: String,
    #[serde(alias = "image_paths", skip_serializing_if = "Vec::is_empty", default)]
    pub image_paths: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GenerateImageResultStep {
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
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SkillLookupCallStep {
    #[serde(
        with = "crate::proto::serde_helpers::ProtoEnumField::<skill_lookup_call_step::Operation>",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub operation: i32,
    #[serde(
        alias = "requested_skill_names",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub requested_skill_names: Vec<String>,
}
/// Nested message and enum types in `SkillLookupCallStep`.
pub mod skill_lookup_call_step {
    crate::proto::serde_helpers::define_proto_enum! {
        pub enum Operation {
            #[default]
            Unspecified = 0 => "OPERATION_UNSPECIFIED",
            ListSkills = 1 => "OPERATION_LIST_SKILLS",
            LookupSkills = 2 => "OPERATION_LOOKUP_SKILLS",
            GetSkillResources = 3 => "OPERATION_GET_SKILL_RESOURCES",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SkillLookupResultStep {
    #[serde(
        alias = "resolved_skill_names",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub resolved_skill_names: Vec<String>,
    #[serde(
        alias = "error_message",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub error_message: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ExperimentalToolCallStep {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub name: String,
    #[serde(
        alias = "internal_data",
        with = "crate::proto::serde_helpers::base64_bytes",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub internal_data: Vec<u8>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ExperimentalToolResultStep {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub name: String,
    #[serde(
        alias = "internal_data",
        with = "crate::proto::serde_helpers::base64_bytes",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub internal_data: Vec<u8>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ManageTaskCallStep {
    #[serde(
        with = "crate::proto::serde_helpers::ProtoEnumField::<manage_task_call_step::Action>",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub action: i32,
    #[serde(alias = "call_id", skip_serializing_if = "String::is_empty", default)]
    pub call_id: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub input: String,
}
/// Nested message and enum types in `ManageTaskCallStep`.
pub mod manage_task_call_step {
    crate::proto::serde_helpers::define_proto_enum! {
        pub enum Action {
            #[default]
            Unspecified = 0 => "ACTION_UNSPECIFIED",
            List = 1 => "ACTION_LIST",
            Status = 2 => "ACTION_STATUS",
            SendInput = 3 => "ACTION_SEND_INPUT",
            Kill = 4 => "ACTION_KILL",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ManageTaskResultStep {
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tasks: Vec<manage_task_result_step::Task>,
}
/// Nested message and enum types in `ManageTaskResultStep`.
pub mod manage_task_result_step {
    crate::proto::serde_helpers::define_proto_enum! {
        pub enum Status {
            #[default]
            Unspecified = 0 => "STATUS_UNSPECIFIED",
            Running = 1 => "STATUS_RUNNING",
            Completed = 2 => "STATUS_COMPLETED",
            Failed = 3 => "STATUS_FAILED",
            Cancelled = 4 => "STATUS_CANCELLED",
        }
    }
    #[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct Task {
        #[serde(alias = "call_id", skip_serializing_if = "String::is_empty", default)]
        pub call_id: String,
        #[serde(
            with = "crate::proto::serde_helpers::ProtoEnumField::<Status>",
            skip_serializing_if = "crate::proto::serde_helpers::is_default",
            default
        )]
        pub status: i32,
    }
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FinishCallStep {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub arguments: Option<Struct>,
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase", default)]
pub struct FinishResultStep {}

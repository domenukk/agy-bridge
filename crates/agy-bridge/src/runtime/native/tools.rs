//! Available tools construction for direct backend runtime.

use serde::Serialize;

use super::events::parse_json_or_null;
use crate::{
    config::{AgentConfig, McpServer},
    proto::localharness::StepUpdate,
    tools::{AvailableTool, ToolSource},
    types::ToolCallInfo,
};

/// SDK tool names reported for harness-side builtin step actions (`0.1.24`).
mod tool_names {
    pub(super) const VIEW_FILE: &str = "view_file";
    pub(super) const CREATE_FILE: &str = "create_file";
    pub(super) const EDIT_FILE: &str = "edit_file";
    pub(super) const LIST_DIRECTORY: &str = "list_directory";
    pub(super) const FIND_FILE: &str = "find_file";
    pub(super) const SEARCH_DIRECTORY: &str = "search_directory";
    pub(super) const RUN_COMMAND: &str = "run_command";
    pub(super) const INVOKE_SUBAGENT: &str = "invoke_subagent";
    pub(super) const GENERATE_IMAGE: &str = "generate_image";
    pub(super) const SEARCH_WEB: &str = "search_web";
    pub(super) const READ_URL_CONTENT: &str = "read_url_content";
    pub(super) const SKILL_LOOKUP: &str = "skill_lookup";
    pub(super) const RUN_WORKFLOW: &str = "run_workflow";
}

/// Arguments reported for a `view_file` step.
#[derive(Serialize)]
struct ViewFileArgs<'a> {
    file_path: &'a str,
    start_line: u32,
    end_line: u32,
}

/// Arguments reported for steps addressing a single file.
#[derive(Serialize)]
struct FilePathArgs<'a> {
    file_path: &'a str,
}

/// Arguments reported for steps addressing a single directory.
#[derive(Serialize)]
struct DirectoryPathArgs<'a> {
    directory_path: &'a str,
}

/// Arguments reported for directory searches (`find_by_name`, `grep_search`).
#[derive(Serialize)]
struct DirectoryQueryArgs<'a> {
    directory_path: &'a str,
    query: &'a str,
}

/// Arguments reported for a `run_command` step.
#[derive(Serialize)]
struct RunCommandArgs<'a> {
    command_line: &'a str,
    working_dir: &'a str,
}

/// Arguments reported for a `generate_image` step.
#[derive(Serialize)]
struct GenerateImageArgs<'a> {
    prompt: &'a str,
}

/// Arguments reported for a `search_web` step.
#[derive(Serialize)]
struct SearchWebArgs<'a> {
    query: &'a str,
    domain: &'a str,
}

/// Arguments reported for a `read_url_content` step.
#[derive(Serialize)]
struct ReadUrlArgs<'a> {
    url: &'a str,
}

/// Arguments reported for a `skill_lookup` step (`0.1.24`+).
#[derive(Serialize)]
struct SkillLookupArgs<'a> {
    operation: &'a str,
    requested_skill_names: &'a [String],
    resolved_skill_names: &'a [String],
    #[serde(skip_serializing_if = "str::is_empty")]
    error_message: &'a str,
}

/// Arguments reported for a `run_workflow` step.
#[derive(Serialize)]
struct RunWorkflowArgs<'a> {
    script_path: &'a str,
    script: &'a str,
    description: &'a str,
}

/// Build the initial list of available tools from agent configuration.
#[must_use]
pub fn build_available_tools(config: &AgentConfig) -> Vec<AvailableTool> {
    let mut available_tools = Vec::new();

    for def in &config.tools {
        available_tools.push(AvailableTool {
            name: def.name.clone(),
            description: def.description.clone(),
            parameter_schema: def.parameter_schema.clone(),
            source: ToolSource::Custom,
        });
    }

    let enabled_builtins =
        crate::config::CapabilitiesConfig::resolve_active_builtins(config.capabilities.as_ref());

    for b in enabled_builtins {
        available_tools.push(AvailableTool {
            name: b.as_sdk_name().to_string(),
            description: b.description().to_string(),
            parameter_schema: serde_json::Value::Null,
            source: ToolSource::Builtin,
        });
    }

    for mcp in &config.mcp_servers {
        let name = match mcp {
            McpServer::Stdio(s) => &s.command,
            McpServer::Sse(s) => &s.url,
            McpServer::Http(s) => &s.url,
        };
        available_tools.push(AvailableTool {
            name: name.clone(),
            description: format!("MCP server {name}"),
            parameter_schema: serde_json::Value::Null,
            source: ToolSource::Mcp,
        });
    }

    available_tools
}

/// Serialize strongly-typed step arguments, logging (never swallowing) failures.
fn typed_args<T: Serialize>(tool_name: &str, args: &T) -> serde_json::Value {
    match serde_json::to_value(args) {
        Ok(value) => value,
        Err(err) => {
            tracing::warn!(error = %err, tool = tool_name, "Failed to serialize step tool arguments; reporting null");
            serde_json::Value::Null
        }
    }
}

const WIRE_PATH_ARGUMENT_KEYS: [&str; 10] = [
    "path",
    "file_path",
    "directory_path",
    "TargetFile",
    "AbsolutePath",
    "DirectoryPath",
    "SearchPath",
    "output_path",
    "script_path",
    "ScriptPath",
];

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn percent_decode_uri_path_strict(raw: &str) -> Result<String, String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0;
    while idx < bytes.len() {
        if bytes[idx] == b'%'
            && idx + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex_nibble(bytes[idx + 1]), hex_nibble(bytes[idx + 2]))
        {
            let decoded = (hi << 4) | lo;
            if decoded == 0 {
                return Err("percent-encoded NUL byte (%00) is forbidden in wire path".to_owned());
            }
            out.push(decoded);
            idx += 3;
            continue;
        }
        if bytes[idx] == 0 {
            return Err("raw NUL byte is forbidden in wire path".to_owned());
        }
        out.push(bytes[idx]);
        idx += 1;
    }
    std::str::from_utf8(&out)
        .map(str::to_owned)
        .map_err(|e| format!("invalid UTF-8 after percent-decoding wire path: {e}"))
}

fn strip_ascii_case_prefix<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| s.get(prefix.len()..))
        .flatten()
}

fn contains_percent_nul(s: &str) -> bool {
    s.as_bytes()
        .windows(3)
        .any(|w| w[0] == b'%' && w[1] == b'0' && w[2] == b'0')
}

fn has_non_drive_uri_scheme(s: &str) -> bool {
    let Some((scheme, rest)) = s.split_once(':') else {
        return false;
    };
    if !rest.starts_with("//") {
        return false;
    }
    scheme.len() >= 2
        && scheme
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphabetic())
        && scheme
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
}

fn normalize_filesystem_lexical(raw_path: &str) -> String {
    if raw_path.is_empty() {
        return String::new();
    }
    let normalized = crate::policies::normalize_path(std::path::Path::new(raw_path));
    normalized.to_string_lossy().into_owned()
}

/// Strictly validates and normalizes wire-format `file://` URIs and filesystem paths:
/// - Rejects embedded `\0` or `%00` bytes;
/// - Rejects `?query` and `#fragment` components on `file://` URIs;
/// - Rejects non-`localhost` remote authorities on `file://host/...` URIs;
/// - Percent-decodes UTF-8 strictly and lexically collapses `.` and `..` segments
///   so `file:///ws/%2e%2e/etc/passwd` resolves to `/etc/passwd`.
pub(crate) fn try_normalize_wire_path(path: &str) -> Result<String, String> {
    if path.is_empty() {
        return Ok(String::new());
    }
    if path.contains('\0') || contains_percent_nul(path) {
        return Err(format!("NUL byte in wire path is forbidden: {path:?}"));
    }
    if let Some(after_scheme) = strip_ascii_case_prefix(path, "file://") {
        if after_scheme.contains('?') || after_scheme.contains('#') {
            return Err(format!(
                "query or fragment in file:// URI is forbidden: {path:?}"
            ));
        }
        let raw_url_path = if let Some(rest) = strip_ascii_case_prefix(after_scheme, "localhost/") {
            format!("/{rest}")
        } else if after_scheme.eq_ignore_ascii_case("localhost") {
            "/".to_owned()
        } else if after_scheme.starts_with('/') {
            let trimmed = after_scheme.trim_start_matches('/');
            format!("/{trimmed}")
        } else {
            return Err(format!(
                "non-localhost authority in file:// URI is forbidden: {path:?}"
            ));
        };
        let decoded = percent_decode_uri_path_strict(&raw_url_path)?;
        #[cfg(windows)]
        if let [b'/', drive, b':', ..] = decoded.as_bytes()
            && drive.is_ascii_alphabetic()
        {
            let win_path = decoded[1..].replace('/', "\\");
            return Ok(normalize_filesystem_lexical(&win_path));
        }
        return Ok(normalize_filesystem_lexical(&decoded));
    }
    if has_non_drive_uri_scheme(path) {
        return Ok(path.to_owned());
    }
    Ok(normalize_filesystem_lexical(path))
}

/// Translates wire-format `file://` URIs (`file:///...`) into clean absolute
/// filesystem paths with `.` and `..` collapsed, matching SDK `0.1.24`
/// `google.antigravity.connections.connection.normalize_wire_path`.
#[must_use]
pub(crate) fn normalize_wire_path(path: &str) -> String {
    match try_normalize_wire_path(path) {
        Ok(normalized) => normalized,
        Err(err) => {
            tracing::warn!(error = %err, raw_path = path, "Rejected unsafe wire path; keeping raw input for display");
            path.to_owned()
        }
    }
}

struct StrictJsonValue(serde_json::Value);

impl<'de> serde::Deserialize<'de> for StrictJsonValue {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct StrictVisitor;

        impl<'de> serde::de::Visitor<'de> for StrictVisitor {
            type Value = StrictJsonValue;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("any valid JSON value without duplicate object keys")
            }

            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Self::Value, E> {
                Ok(StrictJsonValue(serde_json::Value::Bool(v)))
            }

            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Self::Value, E> {
                Ok(StrictJsonValue(serde_json::Value::Number(v.into())))
            }

            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Self::Value, E> {
                Ok(StrictJsonValue(serde_json::Value::Number(v.into())))
            }

            fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Self::Value, E> {
                let num = serde_json::Number::from_f64(v)
                    .ok_or_else(|| E::custom("non-finite float in JSON"))?;
                Ok(StrictJsonValue(serde_json::Value::Number(num)))
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(StrictJsonValue(serde_json::Value::String(v.to_owned())))
            }

            fn visit_string<E: serde::de::Error>(self, v: String) -> Result<Self::Value, E> {
                Ok(StrictJsonValue(serde_json::Value::String(v)))
            }

            fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(StrictJsonValue(serde_json::Value::Null))
            }

            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(StrictJsonValue(serde_json::Value::Null))
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                let mut items = Vec::new();
                while let Some(StrictJsonValue(elem)) = seq.next_element::<StrictJsonValue>()? {
                    items.push(elem);
                }
                Ok(StrictJsonValue(serde_json::Value::Array(items)))
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut access: A,
            ) -> Result<Self::Value, A::Error> {
                let mut map = serde_json::Map::new();
                while let Some(key) = access.next_key::<String>()? {
                    if map.contains_key(&key) {
                        return Err(serde::de::Error::custom(format!(
                            "duplicate JSON object key {key:?} in tool arguments"
                        )));
                    }
                    let StrictJsonValue(val) = access.next_value::<StrictJsonValue>()?;
                    map.insert(key, val);
                }
                Ok(StrictJsonValue(serde_json::Value::Object(map)))
            }
        }

        deserializer.deserialize_any(StrictVisitor)
    }
}

/// Normalize all `WIRE_PATH_ARGUMENT_KEYS` (`"path"`, `"file_path"`, `"directory_path"`,
/// `"TargetFile"`, `"output_path"`, `"script_path"`, `"ScriptPath"`) in-place inside `value`,
/// returning the highest-priority normalized `canonical_path`.
pub(crate) fn normalize_json_object_paths_in_place(
    value: &mut serde_json::Value,
) -> Result<Option<String>, String> {
    let Some(map) = value.as_object_mut() else {
        return Ok(None);
    };
    let mut canonical_path: Option<String> = None;
    for key in WIRE_PATH_ARGUMENT_KEYS {
        if let Some(serde_json::Value::String(raw)) = map.get_mut(key)
            && !raw.is_empty()
        {
            let normalized = try_normalize_wire_path(raw)?;
            raw.clone_from(&normalized);
            if canonical_path.is_none() {
                canonical_path = Some(normalized);
            }
        }
    }
    Ok(canonical_path)
}

/// Parsed tool argument payload with wire-format path URIs normalized in-place.
pub(crate) struct NormalizedToolPayload {
    pub(crate) args: serde_json::Value,
    pub(crate) canonical_path: Option<String>,
}

/// Strictly parse `arguments_json` (rejecting duplicate JSON keys at any depth, malformed JSON,
/// `%00`, `?query`/`#fragment`, and remote `file://host/` URIs), normalize all
/// `WIRE_PATH_ARGUMENT_KEYS` in-place, and extract `canonical_path`.
pub(crate) fn try_parse_and_normalize_tool_args(
    arguments_json: &str,
    context: &str,
) -> Result<NormalizedToolPayload, String> {
    if arguments_json.trim().is_empty() {
        return Ok(NormalizedToolPayload {
            args: serde_json::Value::Null,
            canonical_path: None,
        });
    }
    let StrictJsonValue(mut args) = serde_json::from_str::<StrictJsonValue>(arguments_json)
        .map_err(|e| format!("Invalid tool arguments JSON in {context}: {e}"))?;
    let canonical_path = normalize_json_object_paths_in_place(&mut args)?;
    Ok(NormalizedToolPayload {
        args,
        canonical_path,
    })
}

/// Best-effort display variant of [`try_parse_and_normalize_tool_args`] for non-gating
/// event/step telemetry (logs and falls back cleanly on non-JSON inputs).
#[must_use]
pub(crate) fn parse_and_normalize_tool_args(
    arguments_json: &str,
    context: &str,
) -> NormalizedToolPayload {
    if arguments_json.is_empty() {
        return NormalizedToolPayload {
            args: serde_json::Value::Null,
            canonical_path: None,
        };
    }
    match try_parse_and_normalize_tool_args(arguments_json, context) {
        Ok(payload) => payload,
        Err(err) => {
            tracing::trace!(
                error = %err,
                context,
                "Tool arguments failed strict normalization for telemetry display"
            );
            NormalizedToolPayload {
                args: parse_json_or_null(arguments_json, context),
                canonical_path: None,
            }
        }
    }
}

/// Non-empty path as the canonical path of a tool call.
fn canonical(path: &str) -> Option<String> {
    (!path.is_empty()).then(|| path.to_owned())
}

/// Build a builtin tool call record with typed arguments.
fn builtin_call<T: Serialize>(
    name: &str,
    args: &T,
    canonical_path: Option<String>,
) -> ToolCallInfo {
    ToolCallInfo {
        name: name.to_owned(),
        args: typed_args(name, args),
        id: None,
        canonical_path,
    }
}

fn extract_fs_step_tool_calls(update: &StepUpdate, calls: &mut Vec<ToolCallInfo>) {
    if let Some(ref vf) = update.view_file {
        let norm_path = normalize_wire_path(&vf.file_path);
        let args = ViewFileArgs {
            file_path: &norm_path,
            start_line: vf.start_line,
            end_line: vf.end_line,
        };
        calls.push(builtin_call(
            tool_names::VIEW_FILE,
            &args,
            canonical(&norm_path),
        ));
    }
    if let Some(ref cf) = update.create_file {
        let norm_path = normalize_wire_path(&cf.file_path);
        let args = FilePathArgs {
            file_path: &norm_path,
        };
        calls.push(builtin_call(
            tool_names::CREATE_FILE,
            &args,
            canonical(&norm_path),
        ));
    }
    if let Some(ref ef) = update.edit_file {
        let norm_path = normalize_wire_path(&ef.file_path);
        let args = FilePathArgs {
            file_path: &norm_path,
        };
        calls.push(builtin_call(
            tool_names::EDIT_FILE,
            &args,
            canonical(&norm_path),
        ));
    }
    if let Some(ref ld) = update.list_directory {
        let norm_dir = normalize_wire_path(&ld.directory_path);
        let args = DirectoryPathArgs {
            directory_path: &norm_dir,
        };
        calls.push(builtin_call(
            tool_names::LIST_DIRECTORY,
            &args,
            canonical(&norm_dir),
        ));
    }
    if let Some(ref ff) = update.find_file {
        let norm_dir = normalize_wire_path(&ff.directory_path);
        let args = DirectoryQueryArgs {
            directory_path: &norm_dir,
            query: &ff.query,
        };
        calls.push(builtin_call(
            tool_names::FIND_FILE,
            &args,
            canonical(&norm_dir),
        ));
    }
    if let Some(ref sd) = update.search_directory {
        let norm_dir = normalize_wire_path(&sd.directory_path);
        let args = DirectoryQueryArgs {
            directory_path: &norm_dir,
            query: &sd.query,
        };
        calls.push(builtin_call(
            tool_names::SEARCH_DIRECTORY,
            &args,
            canonical(&norm_dir),
        ));
    }
}

fn extract_web_step_tool_calls(update: &StepUpdate, calls: &mut Vec<ToolCallInfo>) {
    if let Some(ref sw) = update.search_web {
        let args = SearchWebArgs {
            query: &sw.query,
            domain: &sw.domain,
        };
        calls.push(builtin_call(tool_names::SEARCH_WEB, &args, None));
    }
    if let Some(ref ru) = update.read_url_content {
        let args = ReadUrlArgs { url: &ru.url };
        calls.push(builtin_call(tool_names::READ_URL_CONTENT, &args, None));
    }
}

pub(crate) fn extract_step_tool_calls(update: &StepUpdate) -> Vec<ToolCallInfo> {
    let mut calls = Vec::new();
    if let Some(ref ct) = update.custom_tool
        && let Some(ref tc) = ct.tool_call
    {
        let fallback_json;
        let args_str = if tc.arguments_json.trim().is_empty() {
            if let Some(ref st) = tc.arguments {
                fallback_json = st.to_json_value().to_string();
                fallback_json.as_str()
            } else {
                ""
            }
        } else {
            tc.arguments_json.as_str()
        };
        let normalized = parse_and_normalize_tool_args(args_str, &tc.name);
        calls.push(ToolCallInfo {
            name: tc.name.clone(),
            args: normalized.args,
            id: (!tc.id.is_empty()).then(|| tc.id.clone()),
            canonical_path: normalized.canonical_path,
        });
    }
    extract_fs_step_tool_calls(update, &mut calls);
    if let Some(ref rc) = update.run_command {
        let norm_dir = normalize_wire_path(&rc.working_dir);
        let args = RunCommandArgs {
            command_line: &rc.command_line,
            working_dir: &norm_dir,
        };
        calls.push(builtin_call(
            tool_names::RUN_COMMAND,
            &args,
            canonical(&norm_dir),
        ));
    }
    if update.invoke_subagent.is_some() {
        calls.push(ToolCallInfo {
            name: tool_names::INVOKE_SUBAGENT.to_owned(),
            args: serde_json::Value::Null,
            id: None,
            canonical_path: None,
        });
    }
    if let Some(ref gi) = update.generate_image {
        let norm_out = normalize_wire_path(&gi.output_path);
        let args = GenerateImageArgs { prompt: &gi.prompt };
        calls.push(builtin_call(
            tool_names::GENERATE_IMAGE,
            &args,
            canonical(&norm_out),
        ));
    }
    if let Some(ref mt) = update.mcp_tool {
        let qualified_name = if mt.server_name.is_empty() || mt.tool_name.contains('/') {
            mt.tool_name.clone()
        } else {
            format!("{}/{}", mt.server_name, mt.tool_name)
        };
        let normalized = parse_and_normalize_tool_args(&mt.arguments_json, &qualified_name);
        calls.push(ToolCallInfo {
            name: qualified_name,
            args: normalized.args,
            id: None,
            canonical_path: normalized.canonical_path,
        });
    }
    extract_web_step_tool_calls(update, &mut calls);
    if let Some(ref sl) = update.skill_lookup {
        let op_str =
            crate::proto::localharness::action_skill_lookup::Operation::try_from(sl.operation)
                .map_or("OPERATION_UNSPECIFIED", |op| op.as_str_name());
        let args = SkillLookupArgs {
            operation: op_str,
            requested_skill_names: &sl.requested_skill_names,
            resolved_skill_names: &sl.resolved_skill_names,
            error_message: &sl.error_message,
        };
        calls.push(builtin_call(tool_names::SKILL_LOOKUP, &args, None));
    }
    if let Some(ref rw) = update.run_workflow {
        let norm_script = normalize_wire_path(&rw.script_path);
        let args = RunWorkflowArgs {
            script_path: &norm_script,
            script: &rw.script,
            description: &rw.description,
        };
        calls.push(builtin_call(
            tool_names::RUN_WORKFLOW,
            &args,
            canonical(&norm_script),
        ));
    }
    calls
}

#[cfg(test)]
mod tests {
    use llm_tool::ToolDefinition;

    use super::*;
    use crate::config::{BuiltinTools, CapabilitiesConfig};

    #[test]
    fn test_build_available_tools_default() {
        let config = AgentConfig::default();
        let tools = build_available_tools(&config);
        assert_eq!(tools.len(), BuiltinTools::default_tools().len());
        for tool in tools {
            assert_eq!(tool.source, ToolSource::Builtin);
        }
    }

    #[test]
    fn test_build_available_tools_custom_and_builtins() {
        let custom_tool = ToolDefinition::new(
            "calculate",
            "Math calculation",
            serde_json::json!({"type": "object"}),
        );
        let config = AgentConfig::builder().tools(vec![custom_tool]).build();
        let tools = build_available_tools(&config);
        assert_eq!(tools.len(), BuiltinTools::default_tools().len() + 1);
        let custom = tools.iter().find(|t| t.name == "calculate").unwrap();
        assert_eq!(custom.source, ToolSource::Custom);
        assert_eq!(custom.description, "Math calculation");
    }

    #[test]
    fn test_build_available_tools_with_enabled_and_disabled_filters() {
        let config_enabled = AgentConfig::builder()
            .capabilities(
                CapabilitiesConfig::builder()
                    .enabled_tools(vec![BuiltinTools::ViewFile])
                    .build(),
            )
            .build();
        let tools_enabled = build_available_tools(&config_enabled);
        assert_eq!(tools_enabled.len(), 1);
        assert_eq!(tools_enabled[0].name, "view_file");

        let config_disabled = AgentConfig::builder()
            .capabilities(
                CapabilitiesConfig::builder()
                    .disabled_tools(vec![BuiltinTools::ViewFile])
                    .build(),
            )
            .build();
        let tools_disabled = build_available_tools(&config_disabled);
        assert_eq!(
            tools_disabled.len(),
            BuiltinTools::default_tools().len() - 1
        );
        assert!(tools_disabled.iter().all(|t| t.name != "view_file"));
    }

    #[test]
    fn test_build_available_tools_with_mcp_servers() {
        let stdio = McpServer::stdio("git-mcp").build();
        let sse = McpServer::sse("http://localhost:8080/sse").build();
        let http = McpServer::http("http://localhost:8080/mcp").build();
        let config = AgentConfig::builder()
            .mcp_servers(vec![stdio, sse, http])
            .build();
        let tools = build_available_tools(&config);
        let mcp_tools: Vec<_> = tools
            .iter()
            .filter(|t| t.source == ToolSource::Mcp)
            .collect();
        assert_eq!(mcp_tools.len(), 3);
        assert!(mcp_tools.iter().any(|t| t.name == "git-mcp"));
        assert!(
            mcp_tools
                .iter()
                .any(|t| t.name == "http://localhost:8080/sse")
        );
        assert!(
            mcp_tools
                .iter()
                .any(|t| t.name == "http://localhost:8080/mcp")
        );
    }

    #[test]
    fn test_normalize_wire_path_and_in_place_arg_normalization() {
        assert_eq!(
            normalize_wire_path("file:///dev/shm/workspace/foo.py"),
            "/dev/shm/workspace/foo.py"
        );
        assert_eq!(
            normalize_wire_path("file://localhost/tmp/with%20space.rs"),
            "/tmp/with space.rs"
        );
        assert_eq!(
            normalize_wire_path("FILE:///var/tmp/workspace/notes.md"),
            "/var/tmp/workspace/notes.md"
        );
        assert_eq!(normalize_wire_path("/tmp/clean-path"), "/tmp/clean-path");

        // Exact 0.1.23 WIRE_PATH_ARGUMENT_KEYS priority order:
        // ("path", "file_path", "directory_path", "TargetFile", "output_path", "script_path", "ScriptPath")
        let p1 = parse_and_normalize_tool_args(
            r#"{"file_path":"file:///b","path":"file:///tmp/a","extra":42}"#,
            "test",
        );
        assert_eq!(p1.canonical_path.as_deref(), Some("/tmp/a"));
        assert_eq!(
            p1.args,
            serde_json::json!({"path": "/tmp/a", "file_path": "/b", "extra": 42})
        );

        let p2 = parse_and_normalize_tool_args(
            r#"{"TargetFile":"file:///d","directory_path":"file:///c"}"#,
            "test",
        );
        assert_eq!(p2.canonical_path.as_deref(), Some("/c"));
        assert_eq!(
            p2.args,
            serde_json::json!({"directory_path": "/c", "TargetFile": "/d"})
        );

        let p3 = parse_and_normalize_tool_args(
            r#"{"output_path":"file:///e","TargetFile":"file:///tmp/d"}"#,
            "test",
        );
        assert_eq!(p3.canonical_path.as_deref(), Some("/tmp/d"));
        assert_eq!(
            p3.args,
            serde_json::json!({"TargetFile": "/tmp/d", "output_path": "/e"})
        );

        let p4 = parse_and_normalize_tool_args(
            r#"{"ScriptPath":"file:///g","script_path":"file:///f","output_path":"file:///e"}"#,
            "test",
        );
        assert_eq!(p4.canonical_path.as_deref(), Some("/e"));
        assert_eq!(
            p4.args,
            serde_json::json!({"output_path": "/e", "script_path": "/f", "ScriptPath": "/g"})
        );

        let p5 = parse_and_normalize_tool_args(r#"{"ScriptPath":"file:///g"}"#, "test");
        assert_eq!(p5.canonical_path.as_deref(), Some("/g"));
        assert_eq!(p5.args, serde_json::json!({"ScriptPath": "/g"}));

        // Explicit null path keys must be preserved verbatim (never stripped).
        let p_null = parse_and_normalize_tool_args(
            r#"{"file_path":null,"ScriptPath":"file:///g","count":3}"#,
            "test",
        );
        assert_eq!(p_null.canonical_path.as_deref(), Some("/g"));
        assert_eq!(
            p_null.args,
            serde_json::json!({"file_path": null, "ScriptPath": "/g", "count": 3})
        );
        let reparsed: serde_json::Value = serde_json::from_str(
            &serde_json::to_string(&p_null.args).expect("serialize normalized args"),
        )
        .expect("valid normalized_json");
        assert_eq!(
            reparsed,
            serde_json::json!({"file_path": null, "ScriptPath": "/g", "count": 3})
        );

        assert_eq!(
            parse_and_normalize_tool_args(r#"{"path":""}"#, "test").canonical_path,
            None
        );
        assert_eq!(
            parse_and_normalize_tool_args("", "test").canonical_path,
            None
        );
        assert_eq!(
            parse_and_normalize_tool_args("not-json", "test").canonical_path,
            None
        );
    }

    #[test]
    fn test_extract_step_tool_calls_normalizes_wire_uris_in_args_and_canonical_path() {
        use crate::proto::localharness::{
            ActionCreateFile, ActionCustomTool, ActionMcpTool, ActionRunCommand, ActionViewFile,
            ToolCall,
        };

        let update = StepUpdate {
            custom_tool: Some(ActionCustomTool {
                tool_call: Some(ToolCall {
                    id: "call_1".to_string(),
                    name: "read_target".to_string(),
                    arguments_json: r#"{"arg1":"val1","file_path":"file:///tmp/target.rs"}"#
                        .to_string(),
                    ..ToolCall::default()
                }),
                ..ActionCustomTool::default()
            }),
            view_file: Some(ActionViewFile {
                file_path: "file:///dev/shm/workspace/foo.py".to_string(),
                start_line: 1,
                end_line: 20,
                content_offset: 0,
            }),
            create_file: Some(ActionCreateFile {
                file_path: "file:///var/tmp/workspace/notes.md".to_string(),
                contents: "hi".to_string(),
            }),
            run_command: Some(ActionRunCommand {
                command_line: "cargo test".to_string(),
                working_dir: "file:///tmp/ws".to_string(),
                ..ActionRunCommand::default()
            }),
            mcp_tool: Some(ActionMcpTool {
                server_name: "ghidra".to_string(),
                tool_name: "analyze".to_string(),
                arguments_json:
                    r#"{"TargetFile":"file:///tmp/bin.elf","ScriptPath":"file:///tmp/script.py"}"#
                        .to_string(),
            }),
            ..StepUpdate::default()
        };
        let calls = extract_step_tool_calls(&update);
        assert_eq!(calls.len(), 5);
        assert_eq!(calls[0].name, "read_target");
        assert_eq!(
            calls[0].args,
            serde_json::json!({"arg1": "val1", "file_path": "/tmp/target.rs"})
        );
        assert_eq!(calls[0].canonical_path.as_deref(), Some("/tmp/target.rs"));

        assert_eq!(calls[1].name, tool_names::VIEW_FILE);
        assert_eq!(calls[1].args["file_path"], "/dev/shm/workspace/foo.py");
        assert_eq!(
            calls[1].canonical_path.as_deref(),
            Some("/dev/shm/workspace/foo.py")
        );

        assert_eq!(calls[2].name, tool_names::CREATE_FILE);
        assert_eq!(calls[2].args["file_path"], "/var/tmp/workspace/notes.md");
        assert_eq!(
            calls[2].canonical_path.as_deref(),
            Some("/var/tmp/workspace/notes.md")
        );

        assert_eq!(calls[3].name, tool_names::RUN_COMMAND);
        assert_eq!(calls[3].args["working_dir"], "/tmp/ws");
        assert_eq!(calls[3].canonical_path.as_deref(), Some("/tmp/ws"));

        assert_eq!(calls[4].name, "ghidra/analyze");
        assert_eq!(
            calls[4].args,
            serde_json::json!({"TargetFile": "/tmp/bin.elf", "ScriptPath": "/tmp/script.py"})
        );
        assert_eq!(calls[4].canonical_path.as_deref(), Some("/tmp/bin.elf"));
    }
}

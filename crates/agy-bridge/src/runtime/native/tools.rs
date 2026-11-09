//! Available tools construction for direct backend runtime.

use serde::Serialize;

use super::events::parse_json_or_null;
use crate::{
    config::{AgentConfig, McpServer},
    proto::localharness::StepUpdate,
    tools::{AvailableTool, ToolSource},
    types::ToolCallInfo,
};

/// SDK tool names reported for harness-side builtin step actions.
mod tool_names {
    pub(super) const VIEW_FILE: &str = "view_file";
    pub(super) const WRITE_TO_FILE: &str = "write_to_file";
    pub(super) const EDIT_FILE: &str = "edit_file";
    pub(super) const LIST_DIR: &str = "list_dir";
    pub(super) const FIND_BY_NAME: &str = "find_by_name";
    pub(super) const GREP_SEARCH: &str = "grep_search";
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

/// Arguments reported for a `skill_lookup` step.
#[derive(Serialize)]
struct SkillLookupArgs<'a> {
    skill_name: &'a str,
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

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn percent_decode_uri_path(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0;
    while idx < bytes.len() {
        if bytes[idx] == b'%'
            && idx + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex_nibble(bytes[idx + 1]), hex_nibble(bytes[idx + 2]))
        {
            out.push((hi << 4) | lo);
            idx += 3;
            continue;
        }
        out.push(bytes[idx]);
        idx += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn strip_ascii_case_prefix<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| s.get(prefix.len()..))
        .flatten()
}

/// Translates wire-format `file://` URIs (`file:///...`) into clean absolute
/// filesystem paths, matching `google.antigravity.connections.connection.normalize_wire_path`.
#[must_use]
pub(crate) fn normalize_wire_path(path: &str) -> String {
    if let Some(after_scheme) = strip_ascii_case_prefix(path, "file://") {
        let raw_url_path = if let Some(rest) = after_scheme.strip_prefix("localhost/") {
            format!("/{rest}")
        } else if after_scheme.starts_with('/') {
            after_scheme.to_owned()
        } else if let Some((_host, rest)) = after_scheme.split_once('/') {
            format!("/{rest}")
        } else {
            return path.to_owned();
        };
        let decoded = percent_decode_uri_path(&raw_url_path);
        #[cfg(windows)]
        if let [b'/', drive, b':', ..] = decoded.as_bytes()
            && drive.is_ascii_alphabetic()
        {
            return decoded[1..].replace('/', "\\");
        }
        return decoded;
    }
    path.to_owned()
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
enum WirePathField {
    Text(String),
    NonString(serde_json::Value),
}

impl WirePathField {
    fn normalize_in_place(&mut self) -> Option<String> {
        match self {
            Self::Text(s) if !s.is_empty() => {
                let normalized = normalize_wire_path(s);
                s.clone_from(&normalized);
                Some(normalized)
            }
            Self::Text(_) | Self::NonString(_) => None,
        }
    }
}

/// Distinguishes an omitted JSON object key (`Absent`) from an explicit JSON value
/// (`Present`, including explicit `null`), preventing `skip_serializing_if` from
/// dropping `"file_path": null` entries on re-serialization.
#[derive(Default)]
enum WireFieldSlot {
    #[default]
    Absent,
    Present(WirePathField),
}

impl WireFieldSlot {
    const fn is_absent(&self) -> bool {
        matches!(self, Self::Absent)
    }

    fn normalize_in_place(&mut self) -> Option<String> {
        match self {
            Self::Absent => None,
            Self::Present(field) => field.normalize_in_place(),
        }
    }
}

impl serde::Serialize for WireFieldSlot {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Absent => serializer.serialize_none(),
            Self::Present(val) => val.serialize(serializer),
        }
    }
}

impl<'de> serde::Deserialize<'de> for WireFieldSlot {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        WirePathField::deserialize(deserializer).map(Self::Present)
    }
}

/// Strongly-typed tool call JSON object matching
/// `google.antigravity.local_connection_config.WIRE_PATH_ARGUMENT_KEYS` (`0.1.23`):
/// `("path", "file_path", "directory_path", "TargetFile", "output_path", "script_path", "ScriptPath")`.
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct NormalizedToolArgs {
    #[serde(default, skip_serializing_if = "WireFieldSlot::is_absent")]
    path: WireFieldSlot,
    #[serde(default, skip_serializing_if = "WireFieldSlot::is_absent")]
    file_path: WireFieldSlot,
    #[serde(default, skip_serializing_if = "WireFieldSlot::is_absent")]
    directory_path: WireFieldSlot,
    #[serde(
        default,
        rename = "TargetFile",
        skip_serializing_if = "WireFieldSlot::is_absent"
    )]
    target_file: WireFieldSlot,
    #[serde(default, skip_serializing_if = "WireFieldSlot::is_absent")]
    output_path: WireFieldSlot,
    #[serde(default, skip_serializing_if = "WireFieldSlot::is_absent")]
    script_path: WireFieldSlot,
    #[serde(
        default,
        rename = "ScriptPath",
        skip_serializing_if = "WireFieldSlot::is_absent"
    )]
    script_path_pascal: WireFieldSlot,
    #[serde(flatten)]
    extra: std::collections::BTreeMap<String, serde_json::Value>,
}

impl NormalizedToolArgs {
    /// Normalize all non-empty wire-path fields in-place and return the highest-priority
    /// canonical path, matching `local_connection_config.normalize_path_arguments`.
    fn normalize_in_place(&mut self) -> Option<String> {
        let mut canonical_path: Option<String> = None;
        for field in [
            &mut self.path,
            &mut self.file_path,
            &mut self.directory_path,
            &mut self.target_file,
            &mut self.output_path,
            &mut self.script_path,
            &mut self.script_path_pascal,
        ] {
            if let Some(normalized) = field.normalize_in_place()
                && canonical_path.is_none()
            {
                canonical_path = Some(normalized);
            }
        }
        canonical_path
    }
}

/// Parsed tool argument payload with wire-format path URIs normalized in-place.
pub(crate) struct NormalizedToolPayload {
    pub(crate) args: serde_json::Value,
    pub(crate) normalized_json: String,
    pub(crate) canonical_path: Option<String>,
}

/// Parse `arguments_json`, normalize all `WIRE_PATH_ARGUMENT_KEYS` (`file:///`)
/// in-place inside the JSON payload, and extract the priority-ordered `canonical_path`.
#[must_use]
pub(crate) fn parse_and_normalize_tool_args(
    arguments_json: &str,
    context: &str,
) -> NormalizedToolPayload {
    if arguments_json.is_empty() {
        return NormalizedToolPayload {
            args: serde_json::Value::Null,
            normalized_json: String::new(),
            canonical_path: None,
        };
    }
    match serde_json::from_str::<NormalizedToolArgs>(arguments_json) {
        Ok(mut parsed) => {
            let canonical_path = parsed.normalize_in_place();
            let normalized_json = match serde_json::to_string(&parsed) {
                Ok(s) => s,
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        context,
                        "Failed to re-serialize normalized tool arguments; keeping raw JSON"
                    );
                    arguments_json.to_owned()
                }
            };
            let args = match serde_json::to_value(&parsed) {
                Ok(v) => v,
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        context,
                        "Failed to convert normalized tool arguments to Value"
                    );
                    parse_json_or_null(arguments_json, context)
                }
            };
            NormalizedToolPayload {
                args,
                normalized_json,
                canonical_path,
            }
        }
        Err(err) => {
            tracing::trace!(
                error = %err,
                context,
                "Tool arguments are not a JSON object for wire-path normalization"
            );
            NormalizedToolPayload {
                args: parse_json_or_null(arguments_json, context),
                normalized_json: arguments_json.to_owned(),
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
            tool_names::WRITE_TO_FILE,
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
            tool_names::LIST_DIR,
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
            tool_names::FIND_BY_NAME,
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
            tool_names::GREP_SEARCH,
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
        let normalized = parse_and_normalize_tool_args(&tc.arguments_json, &tc.name);
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
        let normalized = parse_and_normalize_tool_args(&mt.arguments_json, &mt.tool_name);
        calls.push(ToolCallInfo {
            name: mt.tool_name.clone(),
            args: normalized.args,
            id: None,
            canonical_path: normalized.canonical_path,
        });
    }
    extract_web_step_tool_calls(update, &mut calls);
    if let Some(ref sl) = update.skill_lookup {
        let args = SkillLookupArgs {
            skill_name: &sl.skill_name,
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
        assert_eq!(tools.len(), BuiltinTools::all_tools().len());
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
        assert_eq!(tools.len(), BuiltinTools::all_tools().len() + 1);
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
        assert_eq!(tools_disabled.len(), BuiltinTools::all_tools().len() - 1);
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
        let reparsed: serde_json::Value =
            serde_json::from_str(&p_null.normalized_json).expect("valid normalized_json");
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

        assert_eq!(calls[2].name, tool_names::WRITE_TO_FILE);
        assert_eq!(calls[2].args["file_path"], "/var/tmp/workspace/notes.md");
        assert_eq!(
            calls[2].canonical_path.as_deref(),
            Some("/var/tmp/workspace/notes.md")
        );

        assert_eq!(calls[3].name, tool_names::RUN_COMMAND);
        assert_eq!(calls[3].args["working_dir"], "/tmp/ws");
        assert_eq!(calls[3].canonical_path.as_deref(), Some("/tmp/ws"));

        assert_eq!(calls[4].name, "analyze");
        assert_eq!(
            calls[4].args,
            serde_json::json!({"TargetFile": "/tmp/bin.elf", "ScriptPath": "/tmp/script.py"})
        );
        assert_eq!(calls[4].canonical_path.as_deref(), Some("/tmp/bin.elf"));
    }
}

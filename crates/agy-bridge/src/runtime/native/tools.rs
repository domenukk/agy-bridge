//! Available tools construction for direct backend runtime.

use std::collections::HashSet;

use serde::Serialize;

use super::events::parse_json_or_null;
use crate::{
    config::{AgentConfig, BuiltinTools, McpServer},
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

    let all_builtins = BuiltinTools::all_tools();
    let enabled_builtins: HashSet<BuiltinTools> = if let Some(ref c) = config.capabilities {
        if let Some(ref enabled) = c.enabled_tools {
            enabled.iter().copied().collect()
        } else if let Some(ref disabled) = c.disabled_tools {
            all_builtins
                .iter()
                .copied()
                .filter(|t| !disabled.contains(t))
                .collect()
        } else {
            all_builtins.iter().copied().collect()
        }
    } else {
        all_builtins.iter().copied().collect()
    };

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
        let args = ViewFileArgs {
            file_path: &vf.file_path,
            start_line: vf.start_line,
            end_line: vf.end_line,
        };
        calls.push(builtin_call(
            tool_names::VIEW_FILE,
            &args,
            canonical(&vf.file_path),
        ));
    }
    if let Some(ref cf) = update.create_file {
        let args = FilePathArgs {
            file_path: &cf.file_path,
        };
        calls.push(builtin_call(
            tool_names::WRITE_TO_FILE,
            &args,
            canonical(&cf.file_path),
        ));
    }
    if let Some(ref ef) = update.edit_file {
        let args = FilePathArgs {
            file_path: &ef.file_path,
        };
        calls.push(builtin_call(
            tool_names::EDIT_FILE,
            &args,
            canonical(&ef.file_path),
        ));
    }
    if let Some(ref ld) = update.list_directory {
        let args = DirectoryPathArgs {
            directory_path: &ld.directory_path,
        };
        calls.push(builtin_call(
            tool_names::LIST_DIR,
            &args,
            canonical(&ld.directory_path),
        ));
    }
    if let Some(ref ff) = update.find_file {
        let args = DirectoryQueryArgs {
            directory_path: &ff.directory_path,
            query: &ff.query,
        };
        calls.push(builtin_call(
            tool_names::FIND_BY_NAME,
            &args,
            canonical(&ff.directory_path),
        ));
    }
    if let Some(ref sd) = update.search_directory {
        let args = DirectoryQueryArgs {
            directory_path: &sd.directory_path,
            query: &sd.query,
        };
        calls.push(builtin_call(
            tool_names::GREP_SEARCH,
            &args,
            canonical(&sd.directory_path),
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
        calls.push(ToolCallInfo {
            name: tc.name.clone(),
            args: parse_json_or_null(&tc.arguments_json, &tc.name),
            id: (!tc.id.is_empty()).then(|| tc.id.clone()),
            canonical_path: None,
        });
    }
    extract_fs_step_tool_calls(update, &mut calls);
    if let Some(ref rc) = update.run_command {
        let args = RunCommandArgs {
            command_line: &rc.command_line,
            working_dir: &rc.working_dir,
        };
        calls.push(builtin_call(tool_names::RUN_COMMAND, &args, None));
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
        let args = GenerateImageArgs { prompt: &gi.prompt };
        calls.push(builtin_call(
            tool_names::GENERATE_IMAGE,
            &args,
            canonical(&gi.output_path),
        ));
    }
    if let Some(ref mt) = update.mcp_tool {
        calls.push(ToolCallInfo {
            name: mt.tool_name.clone(),
            args: parse_json_or_null(&mt.arguments_json, &mt.tool_name),
            id: None,
            canonical_path: None,
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
        let args = RunWorkflowArgs {
            script_path: &rw.script_path,
            script: &rw.script,
            description: &rw.description,
        };
        calls.push(builtin_call(
            tool_names::RUN_WORKFLOW,
            &args,
            canonical(&rw.script_path),
        ));
    }
    calls
}

#[cfg(test)]
mod tests {
    use llm_tool::ToolDefinition;

    use super::*;
    use crate::config::CapabilitiesConfig;

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
}

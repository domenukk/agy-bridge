//! Available tools construction for direct backend runtime.

use std::collections::HashSet;

use crate::{
    config::{AgentConfig, BuiltinTools, McpServer},
    tools::{AvailableTool, ToolSource},
};

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
        let custom_tool = ToolDefinition {
            name: "calculate".to_string(),
            description: "Math calculation".to_string(),
            parameter_schema: serde_json::json!({"type": "object"}),
        };
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

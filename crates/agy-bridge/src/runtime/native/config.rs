//! Translation from Rust `AgentConfig` to localharness protobuf messages.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use llm_tool::ToolDefinition;

use crate::{
    config::{
        AgentConfig, BuiltinTools, CapabilitiesConfig, McpServer, McpStdioServer,
        SystemInstructionSection, SystemInstructions,
    },
    hooks::Hooks,
    policies::{PolicyRule, PolicySet},
    proto,
    tools::ToolRegistry,
};

/// Translates an `AgentConfig` into a `proto::localharness::HarnessConfig`.
pub(crate) fn build_harness_config(
    config: &AgentConfig,
    registry: Option<&Arc<ToolRegistry>>,
    hook_runner: Option<&Arc<Hooks>>,
    policy_set: &PolicySet,
) -> proto::localharness::HarnessConfig {
    // NOLINT: empty string default for cascade_id in proto
    let cascade_id = config.conversation_id.clone().unwrap_or_default();
    let system_instructions = config
        .system_instructions
        .as_ref()
        .map(to_proto_system_instructions);
    let tools = build_tool_protos(registry, &config.tools);
    let harness_side_tools = Some(to_proto_harness_side_tools(config.capabilities.as_ref()));
    let workspaces = build_workspace_protos(&config.workspaces);
    let models = build_models_proto(config);
    let mcp_servers = config.mcp_servers.iter().map(to_proto_mcp_server).collect();
    let enabled_hooks = get_enabled_hooks(hook_runner);
    let app_data_dir = resolve_app_data_dir(config.app_data_dir.as_deref());
    let compaction_threshold = extract_compaction_threshold(config.capabilities.as_ref());
    let finish_tool_schema_json = config
        .response_schema
        .as_ref()
        .map(|s| s.as_value().to_string())
        // NOLINT: empty string default for finish_tool_schema_json in proto
        .unwrap_or_default();
    let policy_config = Some(build_policy_config(policy_set));
    let retry_config = config.retry_config.as_ref().map(to_proto_retry_config);
    let budget_config = config.budget_config.as_ref().map(to_proto_budget_config);
    let agent_behavior = config
        .capabilities
        .as_ref()
        .map_or(proto::localharness::AgentBehavior::Autonomous as i32, |c| {
            to_proto_agent_behavior(c.agent_behavior)
        });
    let custom_subagents = to_proto_custom_agents(&config.subagents);
    let skills_paths = config
        .skills
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();

    proto::localharness::HarnessConfig {
        cascade_id,
        session_continuation_mode: 0,
        system_instructions,
        tools,
        harness_side_tools,
        compaction_threshold,
        workspaces,
        skills_paths,
        finish_tool_schema_json,
        initial_trajectory: Vec::new(),
        app_data_dir,
        mcp_servers,
        models,
        enabled_hooks,
        custom_subagents,
        tool_output_truncation: None,
        retry_config,
        policy_config,
        agent_behavior,
        budget_config,
    }
}

fn optional_u32_to_proto_i32(val: Option<u32>) -> i32 {
    match val {
        Some(n) => match i32::try_from(n) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, value = n, "u32 exceeds i32 range in proto conversion");
                i32::MAX
            }
        },
        None => 0,
    }
}

fn optional_usize_to_proto_i32(val: Option<usize>) -> i32 {
    match val {
        Some(n) => match i32::try_from(n) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, value = n, "usize exceeds i32 range in proto conversion");
                i32::MAX
            }
        },
        None => 0,
    }
}

fn optional_u64_to_proto_i64(val: Option<u64>) -> i64 {
    match val {
        Some(n) => match i64::try_from(n) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, value = n, "u64 exceeds i64 range in proto conversion");
                i64::MAX
            }
        },
        None => 0,
    }
}

fn optional_u64_to_proto_u32(val: Option<u64>) -> u32 {
    match val {
        Some(n) => match u32::try_from(n) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, value = n, "u64 exceeds u32 range in proto conversion");
                u32::MAX
            }
        },
        None => 0,
    }
}

fn to_proto_budget_config(
    budget: &crate::config::BudgetConfig,
) -> proto::localharness::BudgetConfig {
    proto::localharness::BudgetConfig {
        max_model_calls: optional_u32_to_proto_i32(budget.max_model_calls),
        max_tool_calls: optional_u32_to_proto_i32(budget.max_tool_calls),
        max_input_tokens: optional_u64_to_proto_i64(budget.max_input_tokens),
        max_output_tokens: optional_u64_to_proto_i64(budget.max_output_tokens),
        max_total_tokens: optional_u64_to_proto_i64(budget.max_total_tokens),
    }
}

fn to_proto_agent_behavior(behavior: crate::config::AgentBehavior) -> i32 {
    match behavior {
        crate::config::AgentBehavior::Autonomous => {
            proto::localharness::AgentBehavior::Autonomous as i32
        }
        crate::config::AgentBehavior::Interactive => {
            proto::localharness::AgentBehavior::Interactive as i32
        }
    }
}

fn to_proto_custom_agents(
    subagents: &[crate::config::SubagentConfig],
) -> Vec<proto::localharness::CustomAgent> {
    subagents
        .iter()
        .map(|sub| {
            let system_instructions = sub
                .system_instructions
                .as_ref()
                .map(to_proto_system_instructions);
            let agent_behavior = sub
                .capabilities
                .as_ref()
                .map_or(proto::localharness::AgentBehavior::Autonomous as i32, |c| {
                    to_proto_agent_behavior(c.agent_behavior)
                });
            let tools = sub
                .tools
                .iter()
                .map(|tool_name| proto::localharness::Tool {
                    name: tool_name.clone(),
                    description: String::new(),
                    parameters_json_schema: String::new(),
                    response_json_schema: String::new(),
                    defer_loading: false,
                })
                .collect();

            proto::localharness::CustomAgent {
                name: sub.name.clone(),
                description: sub.description.clone(),
                system_instructions,
                harness_side_tools: None,
                tools,
                skills_config: None,
                agent_behavior,
            }
        })
        .collect()
}

fn build_tool_protos(
    registry: Option<&Arc<ToolRegistry>>,
    tools: &[ToolDefinition],
) -> Vec<proto::localharness::Tool> {
    if let Some(reg) = registry {
        reg.definitions()
            .into_iter()
            .map(|def| proto::localharness::Tool {
                name: def.name,
                description: def.description,
                parameters_json_schema: def.parameter_schema.to_string(),
                response_json_schema: String::new(),
                defer_loading: false,
            })
            .collect()
    } else {
        tools
            .iter()
            .map(|def| proto::localharness::Tool {
                name: def.name.clone(),
                description: def.description.clone(),
                parameters_json_schema: def.parameter_schema.to_string(),
                response_json_schema: String::new(),
                defer_loading: false,
            })
            .collect()
    }
}

fn build_workspace_protos(workspaces: &[PathBuf]) -> Vec<proto::localharness::Workspace> {
    workspaces
        .iter()
        .map(|p| to_filesystem_workspace(p.as_path()))
        .collect()
}

fn to_filesystem_workspace(path: &Path) -> proto::localharness::Workspace {
    let fs_workspace = proto::localharness::FilesystemWorkspace {
        directory: path.to_string_lossy().to_string(),
    };
    proto::localharness::Workspace {
        workspace_type: Some(
            proto::localharness::workspace::WorkspaceType::FilesystemWorkspace(fs_workspace),
        ),
    }
}

fn resolve_app_data_dir(custom_path: Option<&Path>) -> String {
    custom_path.map_or_else(dirs_or_default_app_data_dir, |p| {
        p.to_string_lossy().to_string()
    })
}

fn extract_compaction_threshold(caps: Option<&CapabilitiesConfig>) -> u32 {
    caps.and_then(|c| c.compaction_threshold)
        // NOLINT: convert usize threshold to u32
        .and_then(|t| u32::try_from(t).ok())
        // NOLINT: default 0 indicates unconfigured compaction threshold
        .unwrap_or(0)
}

fn dirs_or_default_app_data_dir() -> String {
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        home.join(".gemini")
            .join("antigravity")
            .to_string_lossy()
            .to_string()
    } else {
        "/tmp/.gemini/antigravity".to_string()
    }
}

fn to_proto_system_instructions(
    si: &SystemInstructions,
) -> proto::localharness::SystemInstructions {
    match si {
        SystemInstructions::Custom(text) => custom_system_instructions(text),
        SystemInstructions::Templated { identity, sections } => {
            templated_system_instructions(identity.as_deref(), sections)
        }
    }
}

fn custom_text_part(text: String) -> proto::localharness::custom_system_instructions::Part {
    proto::localharness::custom_system_instructions::Part {
        part: Some(proto::localharness::custom_system_instructions::part::Part::Text(text)),
    }
}

fn custom_system_instructions(text: &str) -> proto::localharness::SystemInstructions {
    let custom = proto::localharness::CustomSystemInstructions {
        part: vec![custom_text_part(text.to_string())],
    };
    proto::localharness::SystemInstructions {
        r#type: Some(proto::localharness::system_instructions::Type::Custom(
            custom,
        )),
    }
}

fn templated_system_instructions(
    identity: Option<&str>,
    sections: &[SystemInstructionSection],
) -> proto::localharness::SystemInstructions {
    let appended_sections = sections
        .iter()
        .map(
            |s| proto::localharness::appended_system_instructions::Section {
                title: s.title.clone(),
                content: s.content.clone(),
            },
        )
        .collect();

    let appended = proto::localharness::AppendedSystemInstructions {
        // NOLINT: empty string default for custom_identity in proto
        custom_identity: identity.unwrap_or_default().to_string(),
        appended_sections,
    };

    proto::localharness::SystemInstructions {
        r#type: Some(proto::localharness::system_instructions::Type::Appended(
            appended,
        )),
    }
}

fn to_proto_harness_side_tools(
    caps: Option<&CapabilitiesConfig>,
) -> proto::localharness::HarnessSideTools {
    let all_tools = BuiltinTools::all_tools();
    let enabled_tools: std::collections::HashSet<BuiltinTools> = if let Some(c) = caps {
        if let Some(ref enabled) = c.enabled_tools {
            enabled.iter().copied().collect()
        } else if let Some(ref disabled) = c.disabled_tools {
            all_tools
                .iter()
                .copied()
                .filter(|t| !disabled.contains(t))
                .collect()
        } else {
            all_tools.iter().copied().collect()
        }
    } else {
        all_tools.iter().copied().collect()
    };

    let subagents_enabled = caps.is_none_or(|c| c.enable_subagents)
        && enabled_tools.contains(&BuiltinTools::StartSubagent);
    let max_nesting_depth = optional_usize_to_proto_i32(caps.and_then(|c| c.max_subagent_depth));
    let allowed_subagents = caps
        .and_then(|c| c.allowed_subagents.clone())
        // NOLINT: empty vector default for allowed_subagents in proto
        .unwrap_or_default();
    let max_timeout_ms = optional_u64_to_proto_u32(caps.and_then(|c| c.command_timeout_ms));

    proto::localharness::HarnessSideTools {
        subagents: Some(proto::localharness::SubagentsConfig {
            enabled: subagents_enabled,
            max_nesting_depth,
            allowed_subagents,
        }),
        find: Some(proto::localharness::FindToolConfig {
            enabled: enabled_tools.contains(&BuiltinTools::FindFile),
        }),
        user_questions: Some(proto::localharness::UserQuestionsConfig {
            enabled: enabled_tools.contains(&BuiltinTools::AskQuestion),
        }),
        run_command: Some(proto::localharness::RunCommandToolConfig {
            enabled: enabled_tools.contains(&BuiltinTools::RunCommand),
            max_timeout_ms,
        }),
        file_edit: Some(proto::localharness::FileEditToolConfig {
            enabled: enabled_tools.contains(&BuiltinTools::EditFile),
        }),
        view_file: Some(proto::localharness::ViewFileToolConfig {
            enabled: enabled_tools.contains(&BuiltinTools::ViewFile),
        }),
        write_to_file: Some(proto::localharness::WriteToFileToolConfig {
            enabled: enabled_tools.contains(&BuiltinTools::CreateFile),
        }),
        grep_search: Some(proto::localharness::GrepSearchToolConfig {
            enabled: enabled_tools.contains(&BuiltinTools::SearchDir),
        }),
        list_dir: Some(proto::localharness::ListDirToolConfig {
            enabled: enabled_tools.contains(&BuiltinTools::ListDir),
        }),
        generate_image: Some(proto::localharness::GenerateImageToolConfig {
            enabled: enabled_tools.contains(&BuiltinTools::GenerateImage),
        }),
        search_web: Some(proto::localharness::SearchWebToolConfig {
            enabled: enabled_tools.contains(&BuiltinTools::SearchWeb),
        }),
        read_url_content: Some(proto::localharness::ReadUrlContentToolConfig {
            enabled: enabled_tools.contains(&BuiltinTools::ReadUrlContent),
        }),
        permissions: None,
        tool_search_config: None,
    }
}

fn build_gemini_endpoint(
    base_url: String,
    api_key: String,
) -> proto::localharness::GeminiApiEndpoint {
    proto::localharness::GeminiApiEndpoint {
        base_url,
        http_headers: HashMap::new(),
        api_key,
        options: None,
    }
}

fn build_model_config(
    name: String,
    model_type: proto::localharness::ModelType,
    base_url: String,
    api_key: String,
) -> proto::localharness::ModelConfig {
    let endpoint = build_gemini_endpoint(base_url, api_key);
    proto::localharness::ModelConfig {
        name,
        types: vec![model_type as i32],
        endpoint: Some(proto::localharness::model_config::Endpoint::GeminiApiEndpoint(endpoint)),
    }
}

fn build_models_proto(config: &AgentConfig) -> Vec<proto::localharness::ModelConfig> {
    let model_name = if !config.model.is_empty() {
        config.model.clone()
    } else if let Some(ref g) = config.gemini {
        g.models.default.name.clone()
    } else {
        crate::config::DEFAULT_MODEL.to_string()
    };

    let api_key = config
        .api_key
        .clone()
        .or_else(|| config.gemini.as_ref().and_then(|g| g.api_key.clone()))
        // NOLINT: environment variable is optional
        .or_else(|| std::env::var("GEMINI_API_KEY").ok())
        // NOLINT: empty string default for api_key in proto
        .unwrap_or_default();
    let base_url = config
        .gemini
        .as_ref()
        .and_then(|g| g.base_url.clone())
        // NOLINT: empty string default for base_url in proto
        .unwrap_or_default();

    let text_model = build_model_config(
        model_name,
        proto::localharness::ModelType::Text,
        base_url.clone(),
        api_key.clone(),
    );

    let image_model_name = config.gemini.as_ref().map_or_else(
        || crate::config::DEFAULT_IMAGE_GENERATION_MODEL.to_string(),
        |g| g.models.image_generation.name.clone(),
    );

    let image_model = build_model_config(
        image_model_name,
        proto::localharness::ModelType::Image,
        base_url,
        api_key,
    );

    vec![text_model, image_model]
}

fn stdio_mcp_server(stdio: &McpStdioServer) -> proto::localharness::McpServerConfig {
    proto::localharness::McpServerConfig {
        name: stdio.command.clone(),
        transport: Some(proto::localharness::mcp_server_config::Transport::Stdio(
            proto::localharness::McpStdioTransport {
                command: stdio.command.clone(),
                args: stdio.args.clone(),
                env: HashMap::new(),
            },
        )),
        enabled_tools: Vec::new(),
        disabled_tools: Vec::new(),
        timeout_seconds: 0,
        auth_provider_type: 0,
    }
}

fn http_mcp_server(
    url: String,
    headers: Option<HashMap<String, String>>,
    timeout_seconds: i32,
) -> proto::localharness::McpServerConfig {
    proto::localharness::McpServerConfig {
        name: url.clone(),
        transport: Some(proto::localharness::mcp_server_config::Transport::Http(
            proto::localharness::McpHttpTransport {
                url,
                // NOLINT: empty header map default for McpServer
                headers: headers.unwrap_or_default(),
            },
        )),
        enabled_tools: Vec::new(),
        disabled_tools: Vec::new(),
        timeout_seconds,
        auth_provider_type: 0,
    }
}

fn to_proto_mcp_server(s: &McpServer) -> proto::localharness::McpServerConfig {
    match s {
        McpServer::Stdio(stdio) => stdio_mcp_server(stdio),
        McpServer::Sse(sse) => http_mcp_server(sse.url.clone(), sse.headers.clone(), 0),
        McpServer::Http(http) => {
            let timeout_secs = f64_to_timeout_seconds(http.timeout);
            http_mcp_server(http.url.clone(), http.headers.clone(), timeout_secs)
        }
    }
}

fn f64_to_timeout_seconds(timeout: f64) -> i32 {
    if timeout >= 0.0 && timeout <= f64::from(i32::MAX) {
        let formatted = format!("{:.0}", timeout.floor());
        // NOLINT: parsed from formatted integer string within bounds
        formatted.parse::<i32>().unwrap_or(0)
    } else {
        0
    }
}

fn get_enabled_hooks(hook_runner: Option<&Arc<Hooks>>) -> Vec<i32> {
    let mut hooks = Vec::new();
    if let Some(hr) = hook_runner {
        for entry in hr.entries() {
            let proto_hook = match entry.point {
                crate::hooks::HookPoint::OnSessionStart => {
                    Some(proto::localharness::LifecycleHook::OnSessionStart as i32)
                }
                crate::hooks::HookPoint::OnSessionEnd => {
                    Some(proto::localharness::LifecycleHook::OnSessionEnd as i32)
                }
                crate::hooks::HookPoint::PreTurn => {
                    Some(proto::localharness::LifecycleHook::PreTurn as i32)
                }
                crate::hooks::HookPoint::PostTurn => {
                    Some(proto::localharness::LifecycleHook::PostTurn as i32)
                }
                crate::hooks::HookPoint::PreToolCallDecide => {
                    Some(proto::localharness::LifecycleHook::PreTool as i32)
                }
                crate::hooks::HookPoint::PostToolCall => {
                    Some(proto::localharness::LifecycleHook::PostTool as i32)
                }
                crate::hooks::HookPoint::OnToolError => {
                    Some(proto::localharness::LifecycleHook::OnToolError as i32)
                }
                _ => None,
            };
            if let Some(h) = proto_hook
                && !hooks.contains(&h)
            {
                hooks.push(h);
            }
        }
    }
    hooks
}

fn rule_to_proto(rule: &PolicyRule) -> Option<proto::localharness::PolicyRule> {
    match rule {
        PolicyRule::Allow(tool) => Some(proto::localharness::PolicyRule {
            tool: tool.clone(),
            server_name: String::new(),
            name: format!("allow_{tool}"),
            decision: proto::localharness::PolicyDecision::Allow as i32,
            deny_reason: String::new(),
            is_dynamic: false,
            rule_id: format!("allow_{tool}"),
        }),
        PolicyRule::Deny(tool) => Some(proto::localharness::PolicyRule {
            tool: tool.clone(),
            server_name: String::new(),
            name: format!("deny_{tool}"),
            decision: proto::localharness::PolicyDecision::Deny as i32,
            deny_reason: String::new(),
            is_dynamic: false,
            rule_id: format!("deny_{tool}"),
        }),
        PolicyRule::AllowAll => Some(proto::localharness::PolicyRule {
            tool: "*".to_string(),
            server_name: String::new(),
            name: "allow_all".to_string(),
            decision: proto::localharness::PolicyDecision::Allow as i32,
            deny_reason: String::new(),
            is_dynamic: false,
            rule_id: "allow_all".to_string(),
        }),
        PolicyRule::DenyAll => Some(proto::localharness::PolicyRule {
            tool: "*".to_string(),
            server_name: String::new(),
            name: "deny_all".to_string(),
            decision: proto::localharness::PolicyDecision::Deny as i32,
            deny_reason: String::new(),
            is_dynamic: false,
            rule_id: "deny_all".to_string(),
        }),
        PolicyRule::AskUser { tool, handler_id } => Some(proto::localharness::PolicyRule {
            tool: tool.clone(),
            server_name: String::new(),
            name: format!("ask_user_{tool}"),
            decision: proto::localharness::PolicyDecision::AskUser as i32,
            deny_reason: String::new(),
            is_dynamic: true,
            rule_id: handler_id.clone(),
        }),
        PolicyRule::WorkspaceOnly(_) => None,
    }
}

fn build_policy_config(policy_set: &PolicySet) -> proto::localharness::PolicyConfig {
    let rules = policy_set.iter().filter_map(rule_to_proto).collect();
    proto::localharness::PolicyConfig { rules }
}

fn to_proto_retry_config(retry: &crate::config::RetryConfig) -> proto::localharness::RetryConfig {
    let api_retry = retry
        .api_retry
        .as_ref()
        .map(|r| proto::localharness::ModelApiRetryConfig {
            max_retries: r.max_retries.unwrap_or(3),
            initial_sleep_duration_ms: r.initial_sleep_duration_ms.unwrap_or(1000),
            exponential_multiplier: r.exponential_multiplier.unwrap_or(2.0),
            jitter_range: r.jitter_range.unwrap_or(0.1),
        });

    proto::localharness::RetryConfig {
        api_retry,
        model_output_retry: None,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::{
        config::{
            AgentConfig, BuiltinTools, CapabilitiesConfig, McpServer, McpStdioServer,
            McpStreamableHttpServer, ModelAPIRetryConfig, RetryConfig, SystemInstructions,
        },
        policies::{PolicyRule, PolicySet},
    };

    #[test]
    fn test_build_harness_config_default() {
        let config = AgentConfig::default();
        let policies = PolicySet::new();
        let harness_config = build_harness_config(&config, None, None, &policies);

        assert!(harness_config.system_instructions.is_none());
        assert!(!harness_config.models.is_empty());
        assert_eq!(harness_config.mcp_servers.len(), 0);
        assert_eq!(harness_config.tools.len(), 0);
    }

    #[test]
    fn test_build_harness_config_with_capabilities_and_workspaces() {
        let config = AgentConfig {
            workspaces: vec![PathBuf::from("/tmp/workspace1")],
            skills: vec![PathBuf::from("/tmp/skills")],
            capabilities: Some(CapabilitiesConfig {
                enabled_tools: Some(vec![BuiltinTools::ViewFile, BuiltinTools::RunCommand]),
                disabled_tools: None,
                compaction_threshold: Some(20),
                ..Default::default()
            }),
            ..Default::default()
        };

        let policies = PolicySet::new();
        let harness_config = build_harness_config(&config, None, None, &policies);

        assert_eq!(harness_config.workspaces.len(), 1);
        assert_eq!(harness_config.skills_paths.len(), 1);
        assert_eq!(harness_config.compaction_threshold, 20);

        let side_tools = harness_config
            .harness_side_tools
            .expect("harness_side_tools");
        assert_eq!(side_tools.view_file.as_ref().map(|c| c.enabled), Some(true));
        assert_eq!(
            side_tools.run_command.as_ref().map(|c| c.enabled),
            Some(true)
        );
        assert_eq!(side_tools.list_dir.as_ref().map(|c| c.enabled), Some(false));
    }

    #[test]
    fn test_build_harness_config_with_mcp_servers() {
        let config = AgentConfig {
            mcp_servers: vec![
                McpServer::Stdio(McpStdioServer {
                    command: "echo-server".to_string(),
                    args: vec!["--port".to_string(), "8080".to_string()],
                }),
                McpServer::Http(McpStreamableHttpServer {
                    url: "http://127.0.0.1:9090/mcp".to_string(),
                    headers: None,
                    timeout: 30.0,
                    sse_read_timeout: 60.0,
                    terminate_on_close: true,
                }),
            ],
            ..Default::default()
        };

        let policies = PolicySet::new();
        let harness_config = build_harness_config(&config, None, None, &policies);

        assert_eq!(harness_config.mcp_servers.len(), 2);
    }

    #[test]
    fn test_build_harness_config_with_policies() {
        let config = AgentConfig::default();
        let policies = PolicySet::validated_from(vec![
            PolicyRule::AllowAll,
            PolicyRule::Deny("dangerous_tool".to_string()),
        ])
        .unwrap();

        let harness_config = build_harness_config(&config, None, None, &policies);
        let policy_cfg = harness_config.policy_config.expect("policy_config");
        assert_eq!(policy_cfg.rules.len(), 2);
    }

    #[test]
    fn test_to_proto_retry_config() {
        let retry = RetryConfig {
            api_retry: Some(ModelAPIRetryConfig {
                max_retries: Some(5),
                initial_sleep_duration_ms: Some(500),
                exponential_multiplier: Some(1.5),
                jitter_range: Some(0.2),
            }),
        };

        let proto = to_proto_retry_config(&retry);
        let api = proto.api_retry.expect("api_retry");
        assert_eq!(api.max_retries, 5);
        assert_eq!(api.initial_sleep_duration_ms, 500);
        assert!((api.exponential_multiplier - 1.5).abs() < f64::EPSILON);
        assert!((api.jitter_range - 0.2).abs() < f64::EPSILON);
    }

    #[test]
    fn test_system_instructions_proto() {
        let custom = SystemInstructions::Custom("You are a helpful assistant.".to_string());
        let proto = to_proto_system_instructions(&custom);
        assert!(matches!(
            proto.r#type,
            Some(proto::localharness::system_instructions::Type::Custom(_))
        ));

        let templated = SystemInstructions::Templated {
            identity: Some("TestBot".to_string()),
            sections: Vec::new(),
        };
        let proto_templated = to_proto_system_instructions(&templated);
        assert!(matches!(
            proto_templated.r#type,
            Some(proto::localharness::system_instructions::Type::Appended(_))
        ));
    }
}

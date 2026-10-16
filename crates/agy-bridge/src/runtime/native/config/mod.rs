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
    let mut cascade_id = String::new();
    if let Some(ref cid) = config.conversation_id {
        cascade_id.clone_from(cid);
    }
    let system_instructions = config
        .system_instructions
        .as_ref()
        .map(to_proto_system_instructions);
    let tools = build_tool_protos(registry, &config.tools);
    let has_workspace_only = policy_set
        .iter()
        .any(|r| matches!(r, PolicyRule::WorkspaceOnly(_)));
    let harness_side_tools = Some(to_proto_harness_side_tools(
        config.capabilities.as_ref(),
        has_workspace_only,
    ));
    let workspaces = build_workspace_protos(&config.workspaces);
    let models = build_models_proto(config);
    let mcp_servers = config.mcp_servers.iter().map(to_proto_mcp_server).collect();
    let enabled_hooks = get_enabled_hooks(hook_runner);
    let app_data_dir = resolve_app_data_dir(config.app_data_dir.as_deref());
    let (compaction_config, compaction_threshold) = extract_compaction_config(config);
    let tool_output_truncation = extract_tool_output_truncation(config.capabilities.as_ref());
    let mut finish_tool_schema_json = String::new();
    if let Some(ref schema) = config.response_schema {
        finish_tool_schema_json = schema.as_value().to_string();
    }
    let policy_config = Some(build_policy_config(policy_set));
    let retry_config = config.retry_config.as_ref().map(to_proto_retry_config);
    let budget_config = config.budget_config.as_ref().map(to_proto_budget_config);
    let agent_behavior = config
        .capabilities
        .as_ref()
        .map_or(proto::localharness::AgentBehavior::Autonomous as i32, |c| {
            to_proto_agent_behavior(c.agent_behavior)
        });
    let custom_subagents = to_proto_custom_agents(&config.subagents, &tools);
    let skills_paths = config
        .skills
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    let skills_config = to_proto_skills_config(&config.inline_skills);
    let mcp_max_output_bytes = optional_u32_to_proto_i32(
        config
            .capabilities
            .as_ref()
            .and_then(|c| c.mcp_max_output_bytes),
    );

    proto::localharness::HarnessConfig {
        cascade_id,
        session_continuation_mode: config.session_continuation_mode.map_or(
            0,
            crate::config::agent::SessionContinuationMode::to_proto_i32,
        ),
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
        tool_output_truncation,
        retry_config,
        policy_config,
        agent_behavior,
        budget_config,
        compaction_config,
        skills_config,
        rules_config: None,
        mcp_max_output_bytes,
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

fn optional_usize_to_proto_opt_i32(val: Option<usize>) -> Option<i32> {
    val.map(|n| match i32::try_from(n) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, value = n, "usize exceeds i32 range in proto conversion");
            i32::MAX
        }
    })
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
        scope: budget.scope.to_proto_i32(),
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
        crate::config::AgentBehavior::Minimal => proto::localharness::AgentBehavior::Minimal as i32,
    }
}

fn to_proto_inline_skill(skill: &crate::config::InlineSkill) -> proto::localharness::SkillSource {
    let mut metadata = skill.metadata.clone();
    if !skill.dependent_tools.is_empty() {
        match serde_json::to_string(&skill.dependent_tools) {
            Ok(json) => {
                metadata.insert("dependent_tools".to_string(), json);
            }
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    skill = %skill.name,
                    "failed to serialize inline skill dependent_tools"
                );
            }
        }
    }
    if !skill.dependent_skills.is_empty() {
        match serde_json::to_string(&skill.dependent_skills) {
            Ok(json) => {
                metadata.insert("dependent_skills".to_string(), json);
            }
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    skill = %skill.name,
                    "failed to serialize inline skill dependent_skills"
                );
            }
        }
    }
    proto::localharness::SkillSource {
        source: Some(proto::localharness::skill_source::Source::Skill(
            proto::skill::Skill {
                name: skill.name.clone(),
                skill_id: String::new(),
                description: skill.description.clone(),
                content: skill.content.clone(),
                allowed_tools: skill.allowed_tools.clone(),
                metadata,
                resources: None,
            },
        )),
    }
}

fn to_proto_skills_config(
    inline_skills: &[crate::config::InlineSkill],
) -> Option<proto::localharness::SkillsConfig> {
    if inline_skills.is_empty() {
        return None;
    }
    Some(proto::localharness::SkillsConfig {
        enabled: true,
        skills: inline_skills.iter().map(to_proto_inline_skill).collect(),
    })
}

fn to_proto_subagent_skills_config(
    cfg: &crate::config::SubagentSkillsConfig,
) -> proto::localharness::SubagentSkillsConfig {
    use proto::localharness::subagent_skills_config::{
        Config, InheritConfig, NoneConfig, OverrideConfig,
    };
    let config = match cfg {
        crate::config::SubagentSkillsConfig::Inherit {
            skill_names,
            extra_skills_paths,
        } => Some(Config::InheritConfig(InheritConfig {
            extra_skills_paths: extra_skills_paths
                .iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect(),
            skill_names: skill_names.clone(),
        })),
        crate::config::SubagentSkillsConfig::None => Some(Config::NoneConfig(NoneConfig {})),
        crate::config::SubagentSkillsConfig::Override {
            skills_paths,
            inline_skills,
        } => Some(Config::OverrideConfig(OverrideConfig {
            skills_paths: skills_paths
                .iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect(),
            skill_sources: inline_skills.iter().map(to_proto_inline_skill).collect(),
        })),
    };
    proto::localharness::SubagentSkillsConfig { config }
}

fn to_proto_subagent_harness_side_tools(
    cap: &crate::config::SubagentCapabilities,
) -> proto::localharness::HarnessSideTools {
    let caps = CapabilitiesConfig {
        enable_subagents: cap
            .allowed_subagents
            .as_ref()
            .is_some_and(|v| !v.is_empty())
            || cap
                .enabled_tools
                .as_ref()
                .is_none_or(|v| v.contains(&BuiltinTools::InvokeSubagent)),
        allowed_subagents: cap.allowed_subagents.clone(),
        max_subagent_depth: None,
        enabled_tools: cap.enabled_tools.clone(),
        disabled_tools: cap.disabled_tools.clone(),
        compaction_threshold: None,
        command_timeout_ms: None,
        agent_behavior: cap.agent_behavior,
        run_command_config: cap.run_command_config.clone(),
        tool_output_truncation_config: cap.tool_output_truncation_config,
        ..CapabilitiesConfig::default()
    };
    to_proto_harness_side_tools(Some(&caps), false)
}

fn to_proto_custom_agents(
    subagents: &[crate::config::SubagentConfig],
    available_tools: &[proto::localharness::Tool],
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
            let harness_side_tools = sub
                .capabilities
                .as_ref()
                .map(to_proto_subagent_harness_side_tools);
            let tools = sub
                .tools
                .iter()
                .map(|tool_name| {
                    if let Some(existing) = available_tools.iter().find(|t| t.name == *tool_name) {
                        existing.clone()
                    } else {
                        proto::localharness::Tool {
                            name: tool_name.clone(),
                            description: String::new(),
                            parameters_json_schema: String::new(),
                            response_json_schema: String::new(),
                            defer_loading: false,
                        }
                    }
                })
                .collect();

            proto::localharness::CustomAgent {
                name: sub.name.clone(),
                description: sub.description.clone(),
                system_instructions,
                harness_side_tools,
                tools,
                skills_config: sub
                    .skills_config
                    .as_ref()
                    .map(to_proto_subagent_skills_config),
                agent_behavior,
                model: sub.model.as_ref().filter(|m| !m.is_empty()).map(|m| {
                    proto::localharness::ModelConfig {
                        name: m.clone(),
                        types: Vec::new(),
                        endpoint: None,
                    }
                }),
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

fn extract_compaction_config(
    config: &crate::config::AgentConfig,
) -> (Option<proto::localharness::CompactionConfig>, u32) {
    if let Some(ref cc) = config.compaction_config {
        let mut proto = proto::localharness::CompactionConfig {
            use_model_reported_token_count: cc.use_model_reported_token_count,
            ..proto::localharness::CompactionConfig::default()
        };
        if let Some(threshold) = cc.token_threshold {
            proto.token_threshold = threshold;
        }
        if let Some(interval) = cc.checkpoint_interval_tokens {
            proto.checkpoint_interval_tokens = interval;
        }
        if let Some(max_ctx) = cc.max_context_tokens {
            proto.max_context_tokens = max_ctx;
        }
        if let Some(ref prompt) = cc.summary_prompt_override {
            proto.summary_prompt_override.clone_from(prompt);
        }
        let threshold = proto.token_threshold;
        (Some(proto), threshold)
    } else if let Some(threshold) = config
        .capabilities
        .as_ref()
        .and_then(|c| c.compaction_threshold)
        .and_then(|t| match u32::try_from(t) {
            Ok(val) => Some(val),
            Err(e) => {
                tracing::debug!(error = %e, "compaction_threshold exceeds u32::MAX; ignoring");
                None
            }
        })
    {
        let proto = proto::localharness::CompactionConfig {
            token_threshold: threshold,
            checkpoint_interval_tokens: 0,
            max_context_tokens: 0,
            summary_prompt_override: String::new(),
            use_model_reported_token_count: None,
        };
        (Some(proto), threshold)
    } else {
        (None, 0)
    }
}

fn extract_tool_output_truncation(
    caps: Option<&CapabilitiesConfig>,
) -> Option<proto::localharness::ToolOutputTruncation> {
    caps.and_then(|c| c.tool_output_truncation_config.as_ref())
        .map(|trunc| proto::localharness::ToolOutputTruncation {
            strategy: Some(
                proto::localharness::tool_output_truncation::Strategy::Truncate(
                    proto::localharness::tool_output_truncation::TruncateStrategy {
                        max_tokens: match i32::try_from(trunc.max_tokens) {
                            Ok(v) => v,
                            Err(e) => {
                                tracing::debug!(error = %e, "truncation max_tokens exceeds i32::MAX; saturating");
                                i32::MAX
                            }
                        },
                        save_full_output: trunc.save_full_output,
                    },
                ),
            ),
        })
}

fn dirs_or_default_app_data_dir() -> String {
    let base = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map_or_else(std::env::temp_dir, PathBuf::from);
    base.join(".gemini")
        .join("antigravity")
        .to_string_lossy()
        .to_string()
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

    let mut appended = proto::localharness::AppendedSystemInstructions {
        appended_sections,
        ..proto::localharness::AppendedSystemInstructions::default()
    };
    if let Some(id) = identity {
        appended.custom_identity = id.to_string();
    }

    proto::localharness::SystemInstructions {
        r#type: Some(proto::localharness::system_instructions::Type::Appended(
            appended,
        )),
    }
}

fn timeout_seconds_to_ms(s: f64) -> Option<u64> {
    match std::time::Duration::try_from_secs_f64(s) {
        Ok(d) => match u64::try_from(d.as_millis()) {
            Ok(ms) => Some(ms),
            Err(err) => {
                tracing::warn!(error = %err, seconds = s, "timeout_seconds exceeds u64 milliseconds");
                None
            }
        },
        Err(err) => {
            tracing::warn!(error = %err, seconds = s, "invalid timeout_seconds float value");
            None
        }
    }
}

fn to_proto_harness_side_tools(
    caps: Option<&CapabilitiesConfig>,
    enforce_workspace_validation: bool,
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

    let subagents_flag = caps.is_none_or(|c| c.enable_subagents);
    let subagents_enabled = subagents_flag && enabled_tools.contains(&BuiltinTools::InvokeSubagent);
    let run_workflow_enabled = subagents_flag && enabled_tools.contains(&BuiltinTools::RunWorkflow);
    let max_nesting_depth =
        optional_usize_to_proto_opt_i32(caps.and_then(|c| c.max_subagent_depth));
    let mut allowed_subagents = Vec::new();
    if let Some(c) = caps
        && let Some(ref allowed) = c.allowed_subagents
    {
        allowed_subagents.clone_from(allowed);
    }
    let max_timeout_ms = optional_u64_to_proto_u32(caps.and_then(|c| {
        c.run_command_config
            .as_ref()
            .and_then(|rc| rc.timeout_seconds)
            .and_then(timeout_seconds_to_ms)
            .or(c.command_timeout_ms)
    }));

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
            enable_daemon_commands: caps
                .and_then(|c| c.run_command_config.as_ref())
                .is_some_and(|rc| rc.enable_daemons),
            enable_sandbox: caps
                .and_then(|c| c.run_command_config.as_ref())
                .is_some_and(|rc| rc.enable_sandbox),
            execution_mode: 0,
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
        permissions: enforce_workspace_validation.then_some(
            proto::localharness::PermissionsConfig {
                enforce_workspace_validation: true,
            },
        ),
        tool_search_config: None,
        manage_task: Some(proto::localharness::ManageTaskToolConfig {
            enabled: enabled_tools.contains(&BuiltinTools::RunCommand)
                || enabled_tools.contains(&BuiltinTools::Schedule),
        }),
        schedule: Some(proto::localharness::ScheduleToolConfig {
            enabled: enabled_tools.contains(&BuiltinTools::Schedule),
        }),
        run_workflow: Some(proto::localharness::RunWorkflowToolConfig {
            enabled: run_workflow_enabled,
        }),
    }
}

fn to_proto_gemini_options(
    gen_cfg: Option<&crate::config::GenerationConfig>,
) -> Option<proto::localharness::GeminiModelOptions> {
    let g = gen_cfg?;
    if g.thinking_level.is_none() && g.service_tier.is_none() && g.max_output_tokens.is_none() {
        return None;
    }
    let mut opts = proto::localharness::GeminiModelOptions::default();
    if let Some(t) = g.thinking_level {
        opts.thinking_level = t.as_str().to_string();
    }
    if let Some(s) = g.service_tier {
        opts.service_tier = s.as_str().to_string();
    }
    if let Some(max_tokens) = g.max_output_tokens {
        opts.max_output_tokens = max_tokens;
    }
    Some(opts)
}

fn build_gemini_endpoint(
    base_url: String,
    api_key: String,
    options: Option<proto::localharness::GeminiModelOptions>,
) -> proto::localharness::GeminiApiEndpoint {
    proto::localharness::GeminiApiEndpoint {
        base_url,
        http_headers: HashMap::new(),
        api_key,
        options,
    }
}

fn build_model_config(
    name: String,
    model_type: proto::localharness::ModelType,
    base_url: String,
    api_key: String,
    options: Option<proto::localharness::GeminiModelOptions>,
) -> proto::localharness::ModelConfig {
    let endpoint = build_gemini_endpoint(base_url, api_key, options);
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

    let mut base_url = String::new();
    if let Some(url) = config.effective_base_url() {
        base_url = url;
    }
    let mut fallback_api_key = String::new();
    if let Some(key) = config
        .effective_api_key()
        .or_else(|| (!base_url.is_empty()).then(|| crate::config::PROXY_AUTH_SENTINEL.to_string()))
    {
        fallback_api_key = key;
    }

    let text_api_key = config
        .gemini
        .as_ref()
        .and_then(|g| g.models.default.api_key.clone())
        .filter(|k| !k.is_empty())
        .unwrap_or_else(|| fallback_api_key.clone());
    let text_options =
        to_proto_gemini_options(config.gemini.as_ref().map(|g| &g.models.default.generation));

    let text_model = build_model_config(
        model_name,
        proto::localharness::ModelType::Text,
        base_url.clone(),
        text_api_key,
        text_options,
    );

    let image_model_name = config.gemini.as_ref().map_or_else(
        || crate::config::DEFAULT_IMAGE_GENERATION_MODEL.to_string(),
        |g| g.models.image_generation.name.clone(),
    );
    let image_api_key = config
        .gemini
        .as_ref()
        .and_then(|g| g.models.image_generation.api_key.clone())
        .filter(|k| !k.is_empty())
        .unwrap_or(fallback_api_key);
    let image_options = to_proto_gemini_options(
        config
            .gemini
            .as_ref()
            .map(|g| &g.models.image_generation.generation),
    );

    let image_model = build_model_config(
        image_model_name,
        proto::localharness::ModelType::Image,
        base_url,
        image_api_key,
        image_options,
    );

    vec![text_model, image_model]
}

fn sanitize_mcp_server_name(raw: &str) -> String {
    let base = std::path::Path::new(raw)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(raw);
    let sanitized: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "mcp_server".to_string()
    } else {
        sanitized
    }
}

fn stdio_mcp_server(stdio: &McpStdioServer) -> proto::localharness::McpServerConfig {
    proto::localharness::McpServerConfig {
        name: sanitize_mcp_server_name(&stdio.command),
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
        force_all_tools_eager: false,
    }
}

fn http_mcp_server(
    url: String,
    headers: Option<&HashMap<String, String>>,
    timeout_seconds: i32,
) -> proto::localharness::McpServerConfig {
    let mut http_transport = proto::localharness::McpHttpTransport {
        url: url.clone(),
        ..proto::localharness::McpHttpTransport::default()
    };
    if let Some(hdrs) = headers {
        http_transport.headers.clone_from(hdrs);
    }
    proto::localharness::McpServerConfig {
        name: url,
        transport: Some(proto::localharness::mcp_server_config::Transport::Http(
            http_transport,
        )),
        enabled_tools: Vec::new(),
        disabled_tools: Vec::new(),
        timeout_seconds,
        auth_provider_type: 0,
        force_all_tools_eager: false,
    }
}

fn to_proto_mcp_server(s: &McpServer) -> proto::localharness::McpServerConfig {
    match s {
        McpServer::Stdio(stdio) => stdio_mcp_server(stdio),
        McpServer::Sse(sse) => http_mcp_server(sse.url.clone(), sse.headers.as_ref(), 0),
        McpServer::Http(http) => {
            let timeout_secs = f64_to_timeout_seconds(http.timeout);
            http_mcp_server(http.url.clone(), http.headers.as_ref(), timeout_secs)
        }
    }
}

fn f64_to_timeout_seconds(timeout: f64) -> i32 {
    if timeout >= 0.0 && timeout <= f64::from(i32::MAX) {
        let formatted = format!("{:.0}", timeout.floor());
        match formatted.parse::<i32>() {
            Ok(v) => v,
            Err(e) => {
                tracing::debug!(error = %e, formatted, "Failed to parse timeout seconds");
                0
            }
        }
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
                crate::hooks::HookPoint::OnCompaction => {
                    Some(proto::localharness::LifecycleHook::OnCompaction as i32)
                }
                crate::hooks::HookPoint::Stop => {
                    Some(proto::localharness::LifecycleHook::Stop as i32)
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
    let has_workspace_only = policy_set
        .iter()
        .any(|r| matches!(r, PolicyRule::WorkspaceOnly(_)));
    let workspace_containment = if has_workspace_only {
        proto::localharness::policy_config::WorkspaceContainment::Enabled as i32
    } else {
        proto::localharness::policy_config::WorkspaceContainment::Unspecified as i32
    };
    proto::localharness::PolicyConfig {
        rules,
        workspace_containment,
        auto_config: None,
    }
}

fn to_proto_retry_config(retry: &crate::config::RetryConfig) -> proto::localharness::RetryConfig {
    let api_retry = retry
        .api_retry
        .as_ref()
        .map(|r| proto::localharness::ModelApiRetryConfig {
            max_retries: r.max_retries,
            initial_sleep_duration_ms: r.initial_sleep_duration_ms,
            exponential_multiplier: r.exponential_multiplier,
            jitter_range: r.jitter_range,
        });

    proto::localharness::RetryConfig {
        api_retry,
        model_output_retry: retry.model_output_retry.as_ref().map(|r| {
            proto::localharness::ModelOutputRetryConfig {
                max_retries: r.max_retries,
            }
        }),
    }
}

#[cfg(test)]
mod tests;

use std::path::PathBuf;

use super::*;
use crate::{
    config::{
        AgentConfig, BuiltinTools, CapabilitiesConfig, CompactionConfig, GeminiConfig,
        GenerationConfig, InlineSkill, McpServer, McpStdioServer, McpStreamableHttpServer,
        ModelAPIRetryConfig, ModelConfig, ModelEntry, ModelOutputRetryConfig, RetryConfig,
        SubagentConfig, SubagentSkillsConfig, SystemInstructions, ToolOutputTruncationConfig,
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
                name: None,
                command: "echo-server".to_string(),
                args: vec!["--port".to_string(), "8080".to_string()],
            }),
            McpServer::Http(McpStreamableHttpServer {
                name: None,
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
    assert_eq!(harness_config.mcp_servers[0].name, "echo-server");
    assert_eq!(harness_config.mcp_servers[1].name, "127_0_0_1_9090");
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
        model_output_retry: Some(ModelOutputRetryConfig {
            max_retries: Some(2),
        }),
    };

    let proto = to_proto_retry_config(&retry);
    let api = proto.api_retry.expect("api_retry");
    assert_eq!(api.max_retries, Some(5));
    assert_eq!(api.initial_sleep_duration_ms, Some(500));
    assert!((api.exponential_multiplier.expect("multiplier") - 1.5).abs() < f64::EPSILON);
    assert!((api.jitter_range.expect("jitter") - 0.2).abs() < f64::EPSILON);
    let output = proto.model_output_retry.expect("model_output_retry");
    assert_eq!(output.max_retries, Some(2));

    let no_retry = to_proto_retry_config(&RetryConfig::no_retries());
    let no_retry_json = serde_json::to_string(&no_retry).expect("serialize no_retry");
    assert!(
        no_retry_json.contains("\"maxRetries\":0"),
        "Expected explicit maxRetries:0 in JSON for RetryConfig::no_retries(), got: {no_retry_json}"
    );
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

#[test]
fn test_v017_minimal_behavior_and_lightweight_config() {
    use crate::config::capabilities::AgentBehavior;

    let minimal_tools = BuiltinTools::minimal();
    assert_eq!(
        minimal_tools,
        [
            BuiltinTools::RunCommand,
            BuiltinTools::ViewFile,
            BuiltinTools::CreateFile,
            BuiltinTools::EditFile,
            BuiltinTools::ListDir,
            BuiltinTools::SearchDir,
        ]
    );
    assert_eq!(BuiltinTools::default_tools().len(), 14);

    let caps = CapabilitiesConfig::minimal();
    assert_eq!(caps.agent_behavior, AgentBehavior::Minimal);
    assert!(!caps.enable_subagents);
    assert_eq!(caps.enabled_tools.as_deref(), Some(minimal_tools));

    let cfg = AgentConfig::default().lightweight();
    let policies = PolicySet::new();
    let harness = build_harness_config(&cfg, None, None, &policies);
    assert_eq!(
        harness.agent_behavior,
        proto::localharness::AgentBehavior::Minimal as i32
    );
    let side_tools = harness.harness_side_tools.expect("harness_side_tools");
    assert!(!side_tools.subagents.unwrap().enabled);
    assert!(!side_tools.find.unwrap().enabled);
    assert!(side_tools.grep_search.unwrap().enabled);
    assert!(side_tools.view_file.unwrap().enabled);
    assert!(side_tools.run_command.unwrap().enabled);
    assert!(!side_tools.generate_image.unwrap().enabled);
    assert!(!side_tools.search_web.unwrap().enabled);
    assert!(side_tools.manage_task.unwrap().enabled);
    assert!(!side_tools.schedule.unwrap().enabled);
    assert!(!side_tools.run_workflow.unwrap().enabled);

    // Default config enables schedule and run_workflow
    let default_harness = build_harness_config(&AgentConfig::default(), None, None, &policies);
    let default_tools = default_harness
        .harness_side_tools
        .expect("default harness_side_tools");
    assert!(default_tools.schedule.unwrap().enabled);
    assert!(default_tools.run_workflow.unwrap().enabled);

    // When RunCommand and Schedule are disabled (e.g. read_only), manage_task & schedule are disabled
    let ro_cfg = AgentConfig {
        capabilities: Some(CapabilitiesConfig::read_only()),
        ..AgentConfig::default()
    };
    let ro_harness = build_harness_config(&ro_cfg, None, None, &policies);
    let ro_tools = ro_harness
        .harness_side_tools
        .expect("ro harness_side_tools");
    assert!(!ro_tools.manage_task.unwrap().enabled);
    assert!(!ro_tools.schedule.unwrap().enabled);
    assert!(!ro_tools.run_workflow.unwrap().enabled);
}

#[test]
fn test_v017_benchmark_retry_config() {
    let bench = RetryConfig::benchmark();
    let proto = to_proto_retry_config(&bench);
    let api = proto.api_retry.expect("api_retry");
    assert_eq!(api.max_retries, Some(u32::MAX));
    assert_eq!(api.initial_sleep_duration_ms, Some(1000));
    assert!(proto.model_output_retry.is_none());
}

#[test]
fn test_v0122_inline_skills_subagent_skills_and_new_fields() {
    let skill = InlineSkill::builder()
        .name("triage")
        .description("Triage crashes")
        .content("# Triage\nInspect stack trace.")
        .allowed_tools(vec!["view_file", "grep_search"])
        .dependent_tools(vec!["view_file"])
        .dependent_skills(vec!["base_skill"])
        .build();

    let subagent = SubagentConfig::builder()
        .name("verifier")
        .description("Verifies findings")
        .model("gemini-2.5-flash")
        .skills_config(SubagentSkillsConfig::Override {
            skills_paths: vec![PathBuf::from("/tmp/sub_skills")],
            inline_skills: vec![skill.clone()],
        })
        .build();

    let config = AgentConfig {
        inline_skills: vec![skill],
        subagents: vec![subagent],
        capabilities: Some(CapabilitiesConfig {
            mcp_max_output_bytes: Some(65_536),
            tool_output_truncation_config: Some(
                ToolOutputTruncationConfig::new(8192).with_save_full_output(true),
            ),
            ..CapabilitiesConfig::default()
        }),
        compaction_config: Some(
            CompactionConfig::one_million()
                .with_summary_prompt_override("Summarize key security findings.")
                .with_use_model_reported_token_count(true),
        ),
        gemini: Some(GeminiConfig {
            api_key: Some("test-key".to_string()),
            base_url: None,
            models: ModelConfig {
                default: ModelEntry {
                    name: "gemini-2.5-pro".to_string(),
                    api_key: None,
                    generation: GenerationConfig {
                        thinking_level: None,
                        service_tier: None,
                        max_output_tokens: Some(32_768),
                    },
                },
                image_generation: ModelEntry::default(),
            },
        }),
        ..AgentConfig::default()
    };

    let policies = PolicySet::new();
    let harness = build_harness_config(&config, None, None, &policies);

    assert_eq!(harness.mcp_max_output_bytes, 65_536);

    let skills_cfg = harness.skills_config.expect("skills_config");
    assert!(skills_cfg.enabled);
    assert_eq!(skills_cfg.skills.len(), 1);

    assert_eq!(harness.custom_subagents.len(), 1);
    let proto_sub = &harness.custom_subagents[0];
    assert_eq!(
        proto_sub.model.as_ref().map(|m| m.name.as_str()),
        Some("gemini-2.5-flash")
    );
    assert!(proto_sub.skills_config.is_some());

    let comp = harness.compaction_config.expect("compaction_config");
    assert_eq!(
        comp.summary_prompt_override,
        "Summarize key security findings."
    );
    assert_eq!(comp.use_model_reported_token_count, Some(true));

    let trunc = harness
        .tool_output_truncation
        .expect("tool_output_truncation");
    match trunc.strategy {
        Some(proto::localharness::tool_output_truncation::Strategy::Truncate(t)) => {
            assert_eq!(t.max_tokens, 8192);
            assert!(t.save_full_output);
        }
        other => panic!("Unexpected truncation strategy: {other:?}"),
    }

    let text_endpoint = harness.models[0].endpoint.as_ref().expect("endpoint");
    match text_endpoint {
        proto::localharness::model_config::Endpoint::GeminiApiEndpoint(ep) => {
            let opts = ep.options.as_ref().expect("options");
            assert_eq!(opts.max_output_tokens, 32_768);
        }
        other => panic!("Unexpected endpoint: {other:?}"),
    }
}

#[test]
fn test_titled_custom_instructions_and_new_proto_fields() {
    let config = AgentConfig {
        system_instructions: Some(SystemInstructions::custom_with_title(
            "Follow security policy.",
            "security_rules",
        )),
        capabilities: Some(CapabilitiesConfig {
            enable_subagents: true,
            disable_define_subagent_tool: true,
            ..CapabilitiesConfig::default()
        }),
        compaction_config: Some(
            CompactionConfig::one_million().with_include_last_user_message(true),
        ),
        ..AgentConfig::default()
    };

    let policies = PolicySet::validated_from(vec![PolicyRule::workspace_only(["/tmp/ws_auto"])])
        .expect("valid policy set");
    let harness = build_harness_config(&config, None, None, &policies);

    let si = harness.system_instructions.expect("system_instructions");
    match si.r#type {
        Some(proto::localharness::system_instructions::Type::Custom(custom)) => {
            assert_eq!(custom.part.len(), 1);
            assert_eq!(custom.part[0].title, "security_rules");
            assert_eq!(
                custom.part[0].part,
                Some(
                    proto::localharness::custom_system_instructions::part::Part::Text(
                        "Follow security policy.".to_string()
                    )
                )
            );
        }
        other => panic!("Expected Custom system instructions, got {other:?}"),
    }

    let sub_cfg = harness
        .harness_side_tools
        .expect("harness_side_tools")
        .subagents
        .expect("subagents");
    assert!(sub_cfg.enabled);
    assert!(sub_cfg.disable_define_subagent_tool);

    let comp = harness.compaction_config.expect("compaction_config");
    assert_eq!(comp.include_last_user_message, Some(true));

    assert_eq!(harness.workspaces.len(), 1);
    match &harness.workspaces[0].workspace_type {
        Some(proto::localharness::workspace::WorkspaceType::FilesystemWorkspace(fs_ws)) => {
            assert_eq!(fs_ws.directory, "/tmp/ws_auto");
        }
        other => panic!("Expected FilesystemWorkspace, got {other:?}"),
    }
}

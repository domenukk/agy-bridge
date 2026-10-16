#[cfg(feature = "python")]
use pyo3::types::PyAnyMethods;

#[cfg(feature = "python")]
use super::super::DEFAULT_IMAGE_GENERATION_MODEL;
use super::{
    super::{
        capabilities::BuiltinTools,
        models::{
            GenerationConfig, ModelConfig, ModelEntry, ThinkingLevel, default_image_model_entry,
        },
    },
    *,
};

#[test]
fn test_roundtrip_serialization() {
    let config = AgentConfig {
        system_instructions: Some(SystemInstructions::Custom("Be helpful".to_string())),
        capabilities: Some(CapabilitiesConfig {
            enable_subagents: true,
            enabled_tools: Some(vec![BuiltinTools::ListDir]),
            compaction_threshold: Some(4000),
            ..CapabilitiesConfig::default()
        }),
        workspaces: vec![PathBuf::from("/tmp")],
        ..AgentConfig::default()
    };

    let json = serde_json::to_string(&config).unwrap();
    let parsed: AgentConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.workspaces.len(), 1);
    assert_eq!(
        parsed.capabilities.unwrap().enabled_tools.unwrap()[0],
        BuiltinTools::ListDir
    );
}

#[test]
fn agent_config_builder_with_gemini() {
    let gemini = GeminiConfig {
        api_key: Some("test-key".to_string()),
        base_url: None,
        models: ModelConfig::default(),
    };
    let config = AgentConfig::builder().gemini(gemini).build();
    let gemini_cfg = config.gemini.expect("gemini should be Some");
    assert_eq!(gemini_cfg.api_key.as_deref(), Some("test-key"));
    assert_eq!(gemini_cfg.models.default.name, DEFAULT_MODEL);
}

#[test]
fn agent_config_builder_gemini_with_thinking_level() {
    let gemini = GeminiConfig {
        api_key: None,
        base_url: None,
        models: ModelConfig {
            default: ModelEntry {
                name: "gemini-3.5-flash".to_string(),
                api_key: None,
                generation: GenerationConfig {
                    thinking_level: Some(ThinkingLevel::High),
                    service_tier: None,
                    max_output_tokens: None,
                },
            },
            image_generation: default_image_model_entry(),
        },
    };
    let config = AgentConfig::builder().gemini(gemini).build();
    let gemini_cfg = config.gemini.expect("gemini should be Some");
    assert_eq!(
        gemini_cfg.models.default.generation.thinking_level,
        Some(ThinkingLevel::High)
    );
    assert_eq!(gemini_cfg.models.default.name, "gemini-3.5-flash");
}

#[test]
fn agent_config_gemini_none_by_default() {
    let config = AgentConfig::default();
    assert!(config.gemini.is_none());
}

#[test]
fn agent_config_gemini_serde_roundtrip() {
    let config = AgentConfig {
        gemini: Some(GeminiConfig {
            api_key: Some("roundtrip-key".to_string()),
            base_url: None,
            models: ModelConfig {
                default: ModelEntry {
                    name: "gemini-3.5-flash".to_string(),
                    api_key: Some("model-key".to_string()),
                    generation: GenerationConfig {
                        thinking_level: Some(ThinkingLevel::Medium),
                        service_tier: None,
                        max_output_tokens: None,
                    },
                },
                image_generation: default_image_model_entry(),
            },
        }),
        ..AgentConfig::default()
    };
    let json = serde_json::to_string(&config).unwrap();
    let parsed: AgentConfig = serde_json::from_str(&json).unwrap();
    let gemini_cfg = parsed.gemini.expect("gemini should survive roundtrip");
    assert_eq!(gemini_cfg.api_key.as_deref(), Some("roundtrip-key"));
    assert_eq!(gemini_cfg.models.default.name, "gemini-3.5-flash");
    assert_eq!(
        gemini_cfg.models.default.api_key.as_deref(),
        Some("model-key")
    );
    assert_eq!(
        gemini_cfg.models.default.generation.thinking_level,
        Some(ThinkingLevel::Medium)
    );
}

#[test]
fn system_instructions_custom_serde() {
    let instr = SystemInstructions::Custom("Be a helpful assistant".to_string());
    let json = serde_json::to_string(&instr).unwrap();
    let parsed: SystemInstructions = serde_json::from_str(&json).unwrap();
    match parsed {
        SystemInstructions::Custom(text) => assert_eq!(text, "Be a helpful assistant"),
        SystemInstructions::Templated { .. } => {
            panic!("Expected Custom, got Templated")
        }
    }
}

#[test]
fn system_instructions_templated_serde() {
    let instr = SystemInstructions::Templated {
        identity: Some("a security analyst".to_string()),
        sections: vec![SystemInstructionSection {
            content: "Always check permissions".to_string(),
            title: "security".to_string(),
        }],
    };
    let json = serde_json::to_string(&instr).unwrap();
    let parsed: SystemInstructions = serde_json::from_str(&json).unwrap();
    match parsed {
        SystemInstructions::Templated { identity, sections } => {
            assert_eq!(identity.as_deref(), Some("a security analyst"));
            assert_eq!(sections.len(), 1);
            assert_eq!(sections[0].content, "Always check permissions");
        }
        SystemInstructions::Custom(_) => {
            panic!("Expected Templated, got Custom")
        }
    }
}

#[test]
fn agent_config_fully_populated_serde() {
    #[derive(serde::Deserialize, schemars::JsonSchema)]
    struct CustomToolParams {}

    struct CustomTool;
    impl crate::tools::RustTool for CustomTool {
        type Params = CustomToolParams;
        const NAME: &'static str = "custom_tool";
        const DESCRIPTION: &'static str = "A custom tool";

        async fn call(
            &self,
            _params: Self::Params,
            _ctx: &crate::tools::ToolContext,
        ) -> Result<crate::tools::ToolOutput, crate::tools::ToolError> {
            Ok("ok".into())
        }
    }

    let config = AgentConfig {
        system_instructions: Some(SystemInstructions::Templated {
            identity: Some("test-identity".to_string()),
            sections: vec![],
        }),
        capabilities: Some(CapabilitiesConfig {
            enable_subagents: true,
            disabled_tools: Some(vec![BuiltinTools::RunCommand]),
            compaction_threshold: Some(1000),
            ..CapabilitiesConfig::default()
        }),
        workspaces: vec![PathBuf::from("/a"), PathBuf::from("/b")],
        tools: vec![crate::tools::definition_of(&CustomTool).unwrap()],
        policies: vec![PolicyRule::DenyAll],
        triggers: vec![TriggerEntry {
            name: "poll".to_owned(),
            config: crate::triggers::TriggerConfig::try_every(std::time::Duration::from_secs(30))
                .unwrap(),
            message_template: "time to poll".to_owned(),
        }],
        hooks: vec![HookEntry {
            name: "pre_gate".to_owned(),
            point: crate::hooks::HookPoint::PreTurn,
            callback_id: "cb_pre".to_owned(),
        }],
        skills: vec![PathBuf::from("/skills/foo")],
        ..AgentConfig::default()
    };
    let json = serde_json::to_string(&config).unwrap();
    let parsed: AgentConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.workspaces.len(), 2);
    assert_eq!(parsed.tools.len(), 1);
    assert_eq!(parsed.policies.len(), 1);
    assert_eq!(parsed.triggers.len(), 1);
    assert_eq!(parsed.hooks.len(), 1);
    assert_eq!(parsed.skills.len(), 1);
}

#[test]
fn agent_config_empty_defaults_serde() {
    let json = r#"{"system_instructions":null}"#;
    let parsed: AgentConfig = serde_json::from_str(json).unwrap();
    assert!(parsed.system_instructions.is_none());
    assert!(parsed.capabilities.is_none());
    assert!(parsed.workspaces.is_empty());
    assert!(parsed.tools.is_empty());
    assert_eq!(
        parsed.policies,
        vec![
            PolicyRule::Deny("run_command".to_string()),
            PolicyRule::AllowAll,
        ]
    );
    assert!(parsed.triggers.is_empty());
    assert!(parsed.hooks.is_empty());
    assert!(parsed.skills.is_empty());

    assert!(parsed.gemini.is_none());
}

#[test]
fn agent_config_all_optional_fields_roundtrip() {
    #[derive(schemars::JsonSchema)]
    struct ResponseObj {
        ok: bool,
    }
    assert!(ResponseObj { ok: true }.ok);

    let config = AgentConfig {
        workspaces: vec![PathBuf::from("/ws")],
        skills: vec![PathBuf::from("/skills/test")],

        conversation_id: Some("conv-123".to_string()),
        save_dir: Some(PathBuf::from("/save")),
        app_data_dir: Some(PathBuf::from("/app")),
        response_schema: Some(JsonSchema::from_schema::<ResponseObj>().unwrap()),
        ..AgentConfig::default()
    };
    let json = serde_json::to_string(&config).unwrap();
    let parsed: AgentConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.workspaces.len(), 1);
    assert_eq!(parsed.conversation_id.as_deref(), Some("conv-123"));
    assert_eq!(parsed.save_dir.as_ref().unwrap(), &PathBuf::from("/save"));
    assert!(parsed.response_schema.is_some());
}

#[test]
fn agent_config_custom_tools_and_builtin_tools_coexist() {
    // Audit 3: Verify that an AgentConfig can carry both custom tool
    // definitions (for the ToolRegistry) AND SDK built-in tools
    // (via CapabilitiesConfig.enabled_tools) at the same time.
    let custom_tool = crate::tools::ToolDefinition::new(
        "my_custom_tool",
        "Does something custom",
        serde_json::json!({"type": "object", "properties": {}}),
    );
    let config = AgentConfig {
        tools: vec![custom_tool],
        capabilities: Some(CapabilitiesConfig {
            enabled_tools: Some(vec![BuiltinTools::ViewFile, BuiltinTools::RunCommand]),
            ..CapabilitiesConfig::default()
        }),
        ..AgentConfig::default()
    };

    // Serialize and deserialize to prove the combined config survives a roundtrip.
    let json = serde_json::to_string(&config).unwrap();
    let parsed: AgentConfig = serde_json::from_str(&json).unwrap();

    // Custom tools are preserved.
    assert_eq!(parsed.tools.len(), 1);
    assert_eq!(parsed.tools[0].name, "my_custom_tool");

    // Built-in tool selection is preserved.
    let caps = parsed.capabilities.as_ref().unwrap();
    let enabled = caps.enabled_tools.as_ref().unwrap();
    assert_eq!(enabled.len(), 2);
    assert!(enabled.contains(&BuiltinTools::ViewFile));
    assert!(enabled.contains(&BuiltinTools::RunCommand));

    // Validate the config is internally consistent.
    assert!(caps.validate().is_ok());
}

#[test]
fn agent_config_custom_tools_only_no_builtins() {
    // Verify custom_tools_only() + custom tools is valid.
    let config = AgentConfig {
        tools: vec![crate::tools::ToolDefinition::new(
            "fetch_data",
            "Fetches data",
            serde_json::json!({"type": "object"}),
        )],
        capabilities: Some(CapabilitiesConfig::custom_tools_only()),
        ..AgentConfig::default()
    };

    let caps = config.capabilities.as_ref().unwrap();
    assert!(caps.enabled_tools.as_ref().unwrap().is_empty());
    assert!(caps.validate().is_ok());
    assert_eq!(config.tools.len(), 1);
}

// ── LocalAgentConfig tests ───────────────────────────────────────

#[test]
fn local_agent_config_default() {
    let config = LocalAgentConfig::default();
    assert_eq!(config.agent.model, DEFAULT_MODEL);
}

#[test]
fn local_agent_config_from_agent_config() {
    let agent_cfg = AgentConfig {
        model: "gemini-3.5-flash".to_string(),
        ..AgentConfig::default()
    };
    let local: LocalAgentConfig = agent_cfg.into();
    assert_eq!(local.agent.model, "gemini-3.5-flash");
}

#[test]
fn local_agent_config_serde_roundtrip() {
    let config = LocalAgentConfig::new(AgentConfig::default());
    let json = serde_json::to_string(&config).unwrap();
    let parsed: LocalAgentConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.agent.model, DEFAULT_MODEL);
}

// ── SDK field name alignment tests ────────────────────────────────
//
// Verify that serde serializes field names to match the Python SDK's
// expected JSON keys.

#[test]
fn skills_serializes_as_skills_paths() {
    let config = AgentConfig::builder()
        .skills(vec![PathBuf::from("/skill/a.md")])
        .build();
    let json = serde_json::to_string(&config).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(
        v.get("skills_paths").is_some(),
        "Expected JSON key 'skills_paths', got: {json}"
    );
    assert!(
        v.get("skills").is_none(),
        "Should not have 'skills' key in JSON"
    );
}

#[test]
fn skills_paths_deserializes_to_skills_field() {
    let json = r#"{"skills_paths": ["/skill/a.md"]}"#;
    let config: AgentConfig = serde_json::from_str(json).unwrap();
    assert_eq!(config.skills.len(), 1);
    assert_eq!(config.skills[0], PathBuf::from("/skill/a.md"));
}

#[test]
fn gemini_serializes_as_gemini_config() {
    let config = AgentConfig::builder()
        .gemini(super::super::GeminiConfig::default())
        .build();
    let json = serde_json::to_string(&config).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(
        v.get("gemini_config").is_some(),
        "Expected JSON key 'gemini_config', got: {json}"
    );
    assert!(
        v.get("gemini").is_none(),
        "Should not have 'gemini' key in JSON"
    );
}

#[test]
fn gemini_config_deserializes_to_gemini_field() {
    let json = r#"{"gemini_config": {"api_key": "test-key"}}"#;
    let config: AgentConfig = serde_json::from_str(json).unwrap();
    assert_eq!(
        config.gemini.as_ref().unwrap().api_key.as_deref(),
        Some("test-key")
    );
}

// ── skip_serializing_if tests ─────────────────────────────────────

#[test]
fn empty_vecs_omitted_from_json() {
    let config = AgentConfig::default();
    let json = serde_json::to_string(&config).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    // These should all be absent when empty.
    for key in &[
        "workspaces",
        "tools",
        "triggers",
        "hooks",
        "skills_paths",
        "mcp_servers",
    ] {
        assert!(
            v.get(key).is_none(),
            "Empty vec field '{key}' should be omitted from JSON, got: {json}"
        );
    }
    // policies should always be present (non-empty default)
    assert!(
        v.get("policies").is_some(),
        "policies should always be serialized"
    );
}

#[test]
fn populated_vecs_included_in_json() {
    let config = AgentConfig::builder()
        .skills(vec![PathBuf::from("/skill.md")])
        .workspaces(vec![PathBuf::from("/ws")])
        .build();
    let json = serde_json::to_string(&config).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(
        v.get("skills_paths").is_some(),
        "Non-empty skills should be present"
    );
    assert!(
        v.get("workspaces").is_some(),
        "Non-empty workspaces should be present"
    );
}

// ── Default policy tests ──────────────────────────────────────────

#[test]
fn default_policies_deny_run_command_allow_rest() {
    let config = AgentConfig::default();
    assert_eq!(config.policies.len(), 2);
    assert_eq!(
        config.policies[0],
        PolicyRule::Deny("run_command".to_string())
    );
    assert_eq!(config.policies[1], PolicyRule::AllowAll);
}

// ── Python SDK mirror tests ────────────────────────────────────────
//
// These tests import the live Python SDK and verify that our Rust
// constants haven't drifted from the canonical Python values.  They
// require `pyo3::Python::initialize()` and a venv with the
// SDK installed.

#[cfg(feature = "python")]
/// Helper: extract a Python module-level attribute as a `String`.
fn py_str_attr(module: &str, attr: &str) -> String {
    pyo3::Python::initialize();
    pyo3::Python::attach(|py| {
        crate::runtime::venv::configure_python_sys_path(py)
            .unwrap_or_else(|e| panic!("Failed to configure python sys.path: {e}"));
        let m = crate::runtime::py_scripts::import_serialized(py, module)
            .unwrap_or_else(|e| panic!("Failed to import {module}: {e}"));
        m.getattr(attr)
            .unwrap_or_else(|e| panic!("Failed to get {module}.{attr}: {e}"))
            .extract::<String>()
            .unwrap_or_else(|e| panic!("Failed to extract {module}.{attr} as String: {e}"))
    })
}

#[test]
#[cfg(feature = "python")]
fn default_model_matches_python_sdk() {
    let py_val = py_str_attr("google.antigravity.models", "DEFAULT_MODEL");
    assert_eq!(
        DEFAULT_MODEL, py_val,
        "Rust DEFAULT_MODEL ({DEFAULT_MODEL}) != Python SDK ({py_val})"
    );
}

#[test]
#[cfg(feature = "python")]
fn default_image_model_matches_python_sdk() {
    let py_val = py_str_attr(
        "google.antigravity.models",
        "DEFAULT_IMAGE_GENERATION_MODEL",
    );
    assert_eq!(
        DEFAULT_IMAGE_GENERATION_MODEL, py_val,
        "Rust DEFAULT_IMAGE_GENERATION_MODEL ({DEFAULT_IMAGE_GENERATION_MODEL}) != Python SDK ({py_val})"
    );
}

// ── effective_api_key tests ──────────────────────────────────────

#[test]
fn effective_api_key_prefers_per_model_key() {
    let config = AgentConfig::builder()
        .api_key("top-level-key")
        .gemini(super::super::GeminiConfig {
            api_key: Some("shared-key".into()),
            base_url: None,
            models: super::super::ModelConfig {
                default: super::super::ModelEntry {
                    name: "gemini-3.5-flash".into(),
                    api_key: Some("per-model-key".into()),
                    generation: super::super::GenerationConfig::default(),
                },
                image_generation: super::super::ModelEntry {
                    name: "imagen-4.0-generate-preview-06-03".into(),
                    api_key: None,
                    generation: super::super::GenerationConfig::default(),
                },
            },
        })
        .build();
    assert_eq!(config.effective_api_key().as_deref(), Some("per-model-key"));
}

#[test]
fn effective_api_key_falls_back_to_gemini_shared_key() {
    let config = AgentConfig::builder()
        .gemini(super::super::GeminiConfig {
            api_key: Some("shared-key".into()),
            ..Default::default()
        })
        .build();
    assert_eq!(config.effective_api_key().as_deref(), Some("shared-key"));
}

#[test]
fn effective_api_key_falls_back_to_top_level() {
    let config = AgentConfig::builder().api_key("top-level-key").build();
    assert_eq!(config.effective_api_key().as_deref(), Some("top-level-key"));
}

#[test]
fn effective_api_key_none_without_any_key() {
    let config = AgentConfig::builder().build();
    let result = config.effective_api_key();
    match crate::env_var(crate::config::ENV_GEMINI_API_KEY) {
        Some(env_key) => assert_eq!(result.as_deref(), Some(env_key.as_str())),
        None => assert!(result.is_none()),
    }
}

#[test]
fn initial_history_empty_by_default() {
    let config = AgentConfig::default();
    assert!(config.initial_history.is_empty());

    // Empty initial_history should be skipped in serialization.
    let json = serde_json::to_string(&config).unwrap();
    assert!(
        !json.contains("initial_history"),
        "empty initial_history should be skipped in JSON"
    );
}

#[test]
fn initial_history_roundtrip() {
    use crate::types::{ConversationMessage, MessageRole};

    let config = AgentConfig::builder()
        .initial_history(vec![
            ConversationMessage {
                role: MessageRole::User,
                content: "Hello".to_string(),
            },
            ConversationMessage {
                role: MessageRole::Model,
                content: "Hi there!".to_string(),
            },
        ])
        .build();

    assert_eq!(config.initial_history.len(), 2);

    let json = serde_json::to_string(&config).unwrap();
    assert!(json.contains("initial_history"));

    let parsed: AgentConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.initial_history.len(), 2);
    assert_eq!(parsed.initial_history[0].role, MessageRole::User);
    assert_eq!(parsed.initial_history[0].content, "Hello");
    assert_eq!(parsed.initial_history[1].role, MessageRole::Model);
    assert_eq!(parsed.initial_history[1].content, "Hi there!");
}

#[test]
fn test_agent_config_with_budget_and_subagents() {
    let config = AgentConfig::builder()
        .budget_config(
            crate::config::BudgetConfig::builder()
                .max_model_calls(15)
                .max_total_tokens(80_000)
                .build(),
        )
        .subagents(vec![
            crate::config::SubagentConfig::builder()
                .name("worker")
                .description("Worker subagent")
                .build(),
        ])
        .build();

    assert!(config.budget_config.is_some());
    assert_eq!(config.budget_config.unwrap().max_model_calls, Some(15));
    assert_eq!(config.subagents.len(), 1);
    assert_eq!(config.subagents[0].name, "worker");

    let json = serde_json::to_string(&config).unwrap();
    assert!(json.contains("budget_config"));
    assert!(json.contains("subagents"));

    let parsed: AgentConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.subagents.len(), 1);
    assert_eq!(parsed.subagents[0].name, "worker");
    assert_eq!(parsed.budget_config.unwrap().max_total_tokens, Some(80_000));
}

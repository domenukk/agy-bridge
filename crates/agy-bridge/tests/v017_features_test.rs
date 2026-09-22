use std::str::FromStr;

use agy_bridge::{
    config::{
        AgentConfig, BudgetConfig, BudgetScope, CapabilitiesConfig, CompactionConfig,
        SubagentCapabilities, SubagentConfig, ToolOutputTruncationConfig,
    },
    tools::ToolEffect,
    types::SandboxStatus,
};

#[test]
fn test_compaction_config_builder_and_defaults() {
    let default_cfg = CompactionConfig::default();
    assert_eq!(default_cfg.token_threshold, None);
    assert_eq!(default_cfg.checkpoint_interval_tokens, None);
    assert_eq!(default_cfg.max_context_tokens, None);

    let custom = CompactionConfig::builder()
        .token_threshold(50_000)
        .checkpoint_interval_tokens(10_000)
        .max_context_tokens(100_000)
        .build();
    assert_eq!(custom.token_threshold, Some(50_000));
    assert_eq!(custom.checkpoint_interval_tokens, Some(10_000));
    assert_eq!(custom.max_context_tokens, Some(100_000));

    let json = serde_json::to_string(&custom).expect("serialize");
    let deserialized: CompactionConfig = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(custom, deserialized);
}

#[test]
fn test_agent_config_with_compaction_config() {
    let compaction = CompactionConfig::builder().token_threshold(75_000).build();
    let agent_cfg = AgentConfig::builder().compaction_config(compaction).build();
    assert_eq!(agent_cfg.compaction_config, Some(compaction));

    let json = serde_json::to_string(&agent_cfg).expect("serialize");
    let deserialized: AgentConfig = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(agent_cfg.compaction_config, deserialized.compaction_config);
}

#[test]
fn test_tool_output_truncation_config_builder_and_defaults() {
    let default_cfg = ToolOutputTruncationConfig::default();
    assert_eq!(default_cfg.max_tokens, 0);

    let custom = ToolOutputTruncationConfig::builder()
        .max_tokens(2048)
        .build();
    assert_eq!(custom.max_tokens, 2048);

    let json = serde_json::to_string(&custom).expect("serialize");
    let deserialized: ToolOutputTruncationConfig =
        serde_json::from_str(&json).expect("deserialize");
    assert_eq!(custom, deserialized);
}

#[test]
fn test_capabilities_config_with_tool_output_truncation() {
    let truncation = ToolOutputTruncationConfig::builder()
        .max_tokens(4096)
        .build();
    let caps = CapabilitiesConfig::builder()
        .tool_output_truncation_config(truncation)
        .build();
    assert_eq!(caps.tool_output_truncation_config, Some(truncation));

    let json = serde_json::to_string(&caps).expect("serialize");
    let deserialized: CapabilitiesConfig = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(caps, deserialized);
}

#[test]
fn test_subagent_capabilities_with_tool_output_truncation() {
    let truncation = ToolOutputTruncationConfig::builder()
        .max_tokens(1024)
        .build();
    let sub_caps = SubagentCapabilities::builder()
        .tool_output_truncation_config(truncation)
        .build();
    assert_eq!(sub_caps.tool_output_truncation_config, Some(truncation));

    let sub = SubagentConfig::builder()
        .name("worker")
        .description("Subagent worker")
        .system_instructions("Do work")
        .capabilities(sub_caps)
        .build();

    let json = serde_json::to_string(&sub).expect("serialize");
    let deserialized: SubagentConfig = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(sub, deserialized);
}

#[test]
fn test_budget_scope_variants_and_serde() {
    assert_eq!(BudgetScope::default(), BudgetScope::Lifetime);
    assert_eq!(BudgetScope::Lifetime.as_str(), "LIFETIME");
    assert_eq!(BudgetScope::ForwardLooking.as_str(), "FORWARD_LOOKING");

    assert_eq!(
        BudgetScope::from_str("LIFETIME").unwrap(),
        BudgetScope::Lifetime
    );
    assert_eq!(
        BudgetScope::from_str("FORWARD_LOOKING").unwrap(),
        BudgetScope::ForwardLooking
    );
    assert_eq!(
        BudgetScope::from_str("lifetime").unwrap(),
        BudgetScope::Lifetime
    );
    assert_eq!(
        BudgetScope::from_str("forward_looking").unwrap(),
        BudgetScope::ForwardLooking
    );

    let budget = BudgetConfig::builder()
        .max_model_calls(10)
        .scope(BudgetScope::ForwardLooking)
        .build();
    assert_eq!(budget.scope, BudgetScope::ForwardLooking);

    let json = serde_json::to_string(&budget).expect("serialize");
    let deserialized: BudgetConfig = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(budget, deserialized);
}

#[test]
fn test_sandbox_status_builder_and_defaults() {
    let default_status = SandboxStatus::default();
    assert!(!default_status.available);
    assert_eq!(default_status.unavailable_reason, None);

    let available_status = SandboxStatus::builder().available(true).build();
    assert!(available_status.available);
    assert_eq!(available_status.unavailable_reason, None);

    let unavailable_status = SandboxStatus::builder()
        .available(false)
        .unavailable_reason("Sandbox disabled on Windows host")
        .build();
    assert!(!unavailable_status.available);
    assert_eq!(
        unavailable_status.unavailable_reason.as_deref(),
        Some("Sandbox disabled on Windows host")
    );

    let json = serde_json::to_string(&unavailable_status).expect("serialize");
    let deserialized: SandboxStatus = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(unavailable_status, deserialized);
}

#[test]
fn test_tool_effect_reexport() {
    assert_eq!(ToolEffect::ReadOnly, ToolEffect::ReadOnly);
    assert_ne!(ToolEffect::ReadOnly, ToolEffect::Mutating);
}

#[cfg(feature = "python")]
fn init_py_sys_path(py: pyo3::Python<'_>) {
    use pyo3::types::PyAnyMethods;

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    let manifest_path = std::path::Path::new(&manifest_dir);
    let venv = if manifest_path.join(".venv").is_dir() {
        manifest_path.join(".venv")
    } else if manifest_path.join("../../.venv").is_dir() {
        manifest_path.join("../../.venv")
    } else {
        manifest_path.join(".venv")
    };

    let sys = py.import("sys").expect("import sys");
    let version_info = sys.getattr("version_info").expect("version_info");
    let major: u32 = version_info
        .getattr("major")
        .expect("major")
        .extract()
        .expect("extract major");
    let minor: u32 = version_info
        .getattr("minor")
        .expect("minor")
        .extract()
        .expect("extract minor");
    let py_version = format!("{major}.{minor}");

    let unix_site_packages = venv
        .join("lib")
        .join(format!("python{py_version}"))
        .join("site-packages");
    let win_site_packages = venv.join("Lib").join("site-packages");
    let site_packages = if unix_site_packages.is_dir() {
        unix_site_packages
    } else {
        win_site_packages
    };

    if site_packages.is_dir() {
        let path = sys.getattr("path").expect("get sys.path");
        path.call_method1("insert", (0, site_packages.to_string_lossy().to_string()))
            .expect("insert sys.path");
    }
}

#[cfg(feature = "python")]
static PYTHON_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
#[cfg(feature = "python")]
fn test_python_sdk_v017_types() {
    use pyo3::types::{PyAnyMethods, PyDictMethods};

    let _guard = PYTHON_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    pyo3::Python::initialize();
    pyo3::Python::attach(|py| {
        init_py_sys_path(py);

        let types_mod = match py.import("google.antigravity.types") {
            Ok(m) => m,
            Err(e) => {
                eprintln!("Skipping: google-antigravity not installed ({e})");
                return;
            }
        };

        // 1. CompactionConfig
        assert!(types_mod.hasattr("CompactionConfig").unwrap());
        let comp_cls = types_mod.getattr("CompactionConfig").unwrap();
        let comp_kwargs = pyo3::types::PyDict::new(py);
        comp_kwargs.set_item("token_threshold", 50_000).unwrap();
        let comp_instance = comp_cls.call((), Some(&comp_kwargs)).unwrap();
        assert_eq!(
            comp_instance
                .getattr("token_threshold")
                .unwrap()
                .extract::<u32>()
                .unwrap(),
            50_000
        );

        // 2. ToolOutputTruncationConfig
        assert!(types_mod.hasattr("ToolOutputTruncationConfig").unwrap());
        let trunc_cls = types_mod.getattr("ToolOutputTruncationConfig").unwrap();
        let trunc_kwargs = pyo3::types::PyDict::new(py);
        trunc_kwargs.set_item("max_tokens", 2048).unwrap();
        let trunc_instance = trunc_cls.call((), Some(&trunc_kwargs)).unwrap();
        assert_eq!(
            trunc_instance
                .getattr("max_tokens")
                .unwrap()
                .extract::<u32>()
                .unwrap(),
            2048
        );

        // 3. BudgetScope enum and BudgetConfig
        assert!(types_mod.hasattr("BudgetScope").unwrap());
        let scope_enum = types_mod.getattr("BudgetScope").unwrap();
        assert!(scope_enum.hasattr("LIFETIME").unwrap());
        assert!(scope_enum.hasattr("FORWARD_LOOKING").unwrap());

        assert!(types_mod.hasattr("BudgetConfig").unwrap());
        let budget_cls = types_mod.getattr("BudgetConfig").unwrap();
        let budget_kwargs = pyo3::types::PyDict::new(py);
        budget_kwargs.set_item("max_model_calls", 10).unwrap();
        budget_kwargs
            .set_item("scope", scope_enum.getattr("FORWARD_LOOKING").unwrap())
            .unwrap();
        let budget_instance = budget_cls.call((), Some(&budget_kwargs)).unwrap();
        let scope_val = budget_instance
            .getattr("scope")
            .unwrap()
            .getattr("value")
            .unwrap()
            .extract::<String>()
            .unwrap();
        assert_eq!(scope_val, "FORWARD_LOOKING");

        // 4. SandboxStatus
        assert!(types_mod.hasattr("SandboxStatus").unwrap());
        let sb_cls = types_mod.getattr("SandboxStatus").unwrap();
        let sb_kwargs = pyo3::types::PyDict::new(py);
        sb_kwargs.set_item("available", false).unwrap();
        sb_kwargs
            .set_item("unavailable_reason", "Windows host lacks bubblewrap")
            .unwrap();
        let sb_instance = sb_cls.call((), Some(&sb_kwargs)).unwrap();
        assert!(
            !sb_instance
                .getattr("available")
                .unwrap()
                .extract::<bool>()
                .unwrap()
        );
        let reason = sb_instance
            .getattr("unavailable_reason")
            .unwrap()
            .extract::<Option<String>>()
            .unwrap();
        assert_eq!(reason.as_deref(), Some("Windows host lacks bubblewrap"));
    });
}

#[test]
fn test_v017_lightweight_compaction_budget_retry_mock_server_turn() {
    use agy_bridge::{
        config::{
            AgentBehavior, BuiltinTools, GeminiConfig, GenerationConfig, ModelConfig, ModelEntry,
            RetryConfig, ServiceTier, ThinkingLevel,
        },
        policies::PolicyRule,
    };
    use agy_bridge_test_support::*;

    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![MockResponse::Text(
            "Lightweight v0.17 agent ready.".into(),
        )])
        .await;

        let mut cfg = AgentConfig {
            gemini: Some(GeminiConfig {
                api_key: Some("mock-v017-key".to_string()),
                base_url: Some(server.base_url()),
                models: ModelConfig {
                    default: ModelEntry {
                        name: "gemini-3-flash-preview".to_string(),
                        api_key: Some("per-model-key".to_string()),
                        generation: GenerationConfig {
                            thinking_level: Some(ThinkingLevel::Low),
                            service_tier: Some(ServiceTier::Flex),
                        },
                    },
                    ..ModelConfig::default()
                },
            }),
            compaction_config: Some(CompactionConfig::builder().token_threshold(50_000).build()),
            budget_config: Some(
                BudgetConfig::builder()
                    .max_model_calls(10)
                    .scope(BudgetScope::ForwardLooking)
                    .build(),
            ),
            retry_config: Some(RetryConfig::benchmark()),
            policies: vec![PolicyRule::AllowAll],
            ..AgentConfig::default()
        }
        .lightweight();

        if let Some(ref mut caps) = cfg.capabilities {
            caps.tool_output_truncation_config = Some(
                ToolOutputTruncationConfig::builder()
                    .max_tokens(4096)
                    .build(),
            );
            assert_eq!(caps.agent_behavior, AgentBehavior::Minimal);
            assert_eq!(caps.enabled_tools.as_deref(), Some(BuiltinTools::minimal()));
        }

        let bridge = shared_bridge();
        let agent = bridge.agent(cfg).await.expect("v0.17 agent created");
        let reply = agent
            .chat_text("ping v0.17")
            .await
            .expect("v0.17 chat succeeds");
        assert!(
            reply.contains("Lightweight v0.17 agent ready."),
            "Unexpected reply: {reply}"
        );
        agent.shutdown().await.expect("shutdown succeeds");
    });
}

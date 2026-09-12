use agy_bridge::{
    AgentBehavior, AgentConfig, BudgetConfig, BuiltinTools, CapabilitiesConfig, Modality,
    ModalityTokenCount, Step, StopReason, SubagentCapabilities, SubagentConfig, UsageMetadata,
};

#[test]
fn test_budget_config_full_lifecycle() {
    let budget = BudgetConfig::builder()
        .max_model_calls(20)
        .max_tool_calls(100)
        .max_input_tokens(500_000)
        .max_output_tokens(50_000)
        .max_total_tokens(550_000)
        .build();

    assert_eq!(budget.max_model_calls, Some(20));
    assert_eq!(budget.max_tool_calls, Some(100));
    assert_eq!(budget.max_input_tokens, Some(500_000));
    assert_eq!(budget.max_output_tokens, Some(50_000));
    assert_eq!(budget.max_total_tokens, Some(550_000));

    let json = serde_json::to_string(&budget).expect("serialize");
    let deserialized: BudgetConfig = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(budget, deserialized);
}

#[test]
fn test_agent_behavior_variants_and_wire() {
    assert_eq!(AgentBehavior::default(), AgentBehavior::Autonomous);
    assert_eq!(AgentBehavior::Autonomous.to_string(), "autonomous");
    assert_eq!(AgentBehavior::Interactive.to_string(), "interactive");

    let json_auto = serde_json::to_string(&AgentBehavior::Autonomous).expect("ser");
    assert_eq!(json_auto, "\"autonomous\"");
    let json_inter = serde_json::to_string(&AgentBehavior::Interactive).expect("ser");
    assert_eq!(json_inter, "\"interactive\"");

    let from_upper_auto: AgentBehavior = serde_json::from_str("\"AUTONOMOUS\"").expect("de");
    assert_eq!(from_upper_auto, AgentBehavior::Autonomous);

    let from_upper_inter: AgentBehavior = serde_json::from_str("\"INTERACTIVE\"").expect("de");
    assert_eq!(from_upper_inter, AgentBehavior::Interactive);
}

#[test]
fn test_subagent_config_and_capabilities() {
    let sub = SubagentConfig::builder()
        .name("code_analyzer")
        .description("Analyzes repository AST")
        .system_instructions("Only perform read-only static analysis.")
        .capabilities(
            SubagentCapabilities::builder()
                .agent_behavior(AgentBehavior::Autonomous)
                .allowed_subagents(vec!["ast_parser".to_string()])
                .enabled_tools(vec![BuiltinTools::ViewFile, BuiltinTools::ListDir])
                .build(),
        )
        .tools(vec!["parse_ast".to_string()])
        .build();

    assert_eq!(sub.name, "code_analyzer");
    assert_eq!(sub.description, "Analyzes repository AST");
    assert_eq!(sub.tools, vec!["parse_ast"]);
    let caps = sub.capabilities.as_ref().expect("caps");
    assert_eq!(caps.agent_behavior, AgentBehavior::Autonomous);
    assert_eq!(caps.allowed_subagents, Some(vec!["ast_parser".to_string()]));
    assert_eq!(
        caps.enabled_tools,
        Some(vec![BuiltinTools::ViewFile, BuiltinTools::ListDir])
    );

    let json = serde_json::to_string(&sub).expect("serialize");
    let parsed: SubagentConfig = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed.name, "code_analyzer");
    assert_eq!(parsed.tools, vec!["parse_ast"]);
}

#[test]
fn test_capabilities_v012_fields() {
    let caps = CapabilitiesConfig::builder()
        .agent_behavior(AgentBehavior::Interactive)
        .max_subagent_depth(4)
        .allowed_subagents(vec!["worker1".to_string(), "worker2".to_string()])
        .command_timeout_ms(120_000)
        .build();

    assert_eq!(caps.agent_behavior, AgentBehavior::Interactive);
    assert_eq!(caps.max_subagent_depth, Some(4));
    assert_eq!(
        caps.allowed_subagents,
        Some(vec!["worker1".to_string(), "worker2".to_string()])
    );
    assert_eq!(caps.command_timeout_ms, Some(120_000));

    let json = serde_json::to_string(&caps).expect("serialize");
    let parsed: CapabilitiesConfig = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed.agent_behavior, AgentBehavior::Interactive);
    assert_eq!(parsed.max_subagent_depth, Some(4));
    assert_eq!(parsed.command_timeout_ms, Some(120_000));
}

#[test]
fn test_step_parent_trajectory_and_depth() {
    let step = Step::builder()
        .id("substep_01")
        .step_index(2)
        .cascade_id("cascade_main")
        .trajectory_id("subagent_traj_1")
        .parent_trajectory_id("cascade_main")
        .depth(1)
        .content("Analyzing files...")
        .build();

    assert_eq!(step.id, "substep_01");
    assert_eq!(step.step_index, 2);
    assert_eq!(step.cascade_id, "cascade_main");
    assert_eq!(step.trajectory_id, "subagent_traj_1");
    assert_eq!(step.parent_trajectory_id, "cascade_main");
    assert_eq!(step.depth, 1);
    assert!(step.is_subagent_step());

    let json = serde_json::to_string(&step).expect("serialize");
    let parsed: Step = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed.parent_trajectory_id, "cascade_main");
    assert_eq!(parsed.depth, 1);
}

#[test]
fn test_stop_reason_enum() {
    let reasons = [
        (StopReason::Unspecified, "UNSPECIFIED"),
        (
            StopReason::MaxModelCallsExceeded,
            "MAX_MODEL_CALLS_EXCEEDED",
        ),
        (StopReason::MaxToolCallsExceeded, "MAX_TOOL_CALLS_EXCEEDED"),
        (
            StopReason::MaxInputTokensExceeded,
            "MAX_INPUT_TOKENS_EXCEEDED",
        ),
        (
            StopReason::MaxOutputTokensExceeded,
            "MAX_OUTPUT_TOKENS_EXCEEDED",
        ),
        (
            StopReason::MaxTotalTokensExceeded,
            "MAX_TOTAL_TOKENS_EXCEEDED",
        ),
        (StopReason::QuotaExhausted, "QUOTA_EXHAUSTED"),
        (StopReason::Unknown, "UNKNOWN"),
    ];

    for (reason, wire) in reasons {
        assert_eq!(reason.to_string(), wire);
        let json = serde_json::to_string(&reason).expect("serialize");
        assert_eq!(json, format!("\"{wire}\""));
        let parsed: StopReason = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, reason);
        let from_str: StopReason = wire.parse().expect("parse");
        assert_eq!(from_str, reason);
    }
}

#[test]
fn test_modality_token_counts_in_usage_metadata() {
    let usage = UsageMetadata {
        prompt_token_count: Some(1500),
        cached_content_token_count: Some(500),
        candidates_token_count: Some(300),
        thoughts_token_count: Some(100),
        total_token_count: Some(1900),
        prompt_tokens_details: vec![
            ModalityTokenCount {
                modality: Modality::Text,
                token_count: 1000,
            },
            ModalityTokenCount {
                modality: Modality::Image,
                token_count: 500,
            },
        ],
        cache_tokens_details: vec![ModalityTokenCount {
            modality: Modality::Text,
            token_count: 500,
        }],
        candidates_tokens_details: vec![ModalityTokenCount {
            modality: Modality::Text,
            token_count: 300,
        }],
        tool_use_prompt_tokens_details: Vec::new(),
        service_tier: None,
    };

    let json = serde_json::to_string(&usage).expect("serialize");
    let parsed: UsageMetadata = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed.prompt_tokens_details.len(), 2);
    assert_eq!(parsed.prompt_tokens_details[0].modality, Modality::Text);
    assert_eq!(parsed.prompt_tokens_details[0].token_count, 1000);
    assert_eq!(parsed.prompt_tokens_details[1].modality, Modality::Image);
    assert_eq!(parsed.prompt_tokens_details[1].token_count, 500);
    assert_eq!(parsed.cache_tokens_details[0].modality, Modality::Text);
    assert_eq!(parsed.cache_tokens_details[0].token_count, 500);
}

#[test]
fn test_agent_config_builder_with_all_012_fields() {
    let config = AgentConfig::builder()
        .budget_config(
            BudgetConfig::builder()
                .max_model_calls(50)
                .max_tool_calls(200)
                .max_total_tokens(1_000_000)
                .build(),
        )
        .capabilities(
            CapabilitiesConfig::builder()
                .agent_behavior(AgentBehavior::Autonomous)
                .max_subagent_depth(3)
                .allowed_subagents(vec!["planner".to_string(), "coder".to_string()])
                .command_timeout_ms(30_000)
                .build(),
        )
        .subagents(vec![
            SubagentConfig::builder()
                .name("planner")
                .description("Plans complex architectural refactorings")
                .system_instructions("Create detailed multi-step plans.")
                .build(),
            SubagentConfig::builder()
                .name("coder")
                .description("Implements code changes according to plan")
                .build(),
        ])
        .build();

    assert!(config.budget_config.is_some());
    assert_eq!(
        config.budget_config.as_ref().unwrap().max_model_calls,
        Some(50)
    );
    assert_eq!(config.subagents.len(), 2);
    assert_eq!(config.subagents[0].name, "planner");
    assert_eq!(config.subagents[1].name, "coder");

    let json = serde_json::to_string(&config).expect("serialize");
    let parsed: AgentConfig = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed.subagents.len(), 2);
    assert_eq!(parsed.subagents[0].name, "planner");
    assert_eq!(parsed.subagents[1].name, "coder");
    assert_eq!(
        parsed.budget_config.as_ref().unwrap().max_total_tokens,
        Some(1_000_000)
    );
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

    let site_packages = venv
        .join("lib")
        .join(format!("python{py_version}"))
        .join("site-packages");

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
fn test_python_sdk_v012_budget_and_behavior_parity() {
    use pyo3::types::{PyAnyMethods, PyDictMethods};

    let _guard = PYTHON_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    pyo3::Python::initialize();
    pyo3::Python::attach(|py| {
        init_py_sys_path(py);

        let tp = py
            .import("google.antigravity.types")
            .expect("import google.antigravity.types");

        // Verify BudgetConfig in Python SDK
        assert!(tp.hasattr("BudgetConfig").expect("hasattr BudgetConfig"));
        let budget_cls = tp.getattr("BudgetConfig").expect("get BudgetConfig");
        let kwargs = pyo3::types::PyDict::new(py);
        kwargs.set_item("max_model_calls", 10).unwrap();
        kwargs.set_item("max_total_tokens", 100_000i64).unwrap();
        let py_budget = budget_cls
            .call((), Some(&kwargs))
            .expect("create BudgetConfig");
        assert_eq!(
            py_budget
                .getattr("max_model_calls")
                .unwrap()
                .extract::<i32>()
                .unwrap(),
            10
        );
        assert_eq!(
            py_budget
                .getattr("max_total_tokens")
                .unwrap()
                .extract::<i64>()
                .unwrap(),
            100_000
        );

        // Verify AgentBehavior in Python SDK
        assert!(tp.hasattr("AgentBehavior").expect("hasattr AgentBehavior"));
        let beh_cls = tp.getattr("AgentBehavior").expect("get AgentBehavior");
        let auto_beh = beh_cls.getattr("AUTONOMOUS").expect("AUTONOMOUS");
        assert_eq!(
            auto_beh
                .getattr("value")
                .unwrap()
                .extract::<String>()
                .unwrap(),
            "autonomous"
        );
        let inter_beh = beh_cls.getattr("INTERACTIVE").expect("INTERACTIVE");
        assert_eq!(
            inter_beh
                .getattr("value")
                .unwrap()
                .extract::<String>()
                .unwrap(),
            "interactive"
        );

        // Verify StopReason in Python SDK
        assert!(tp.hasattr("StopReason").expect("hasattr StopReason"));
    });
}

#[test]
#[cfg(feature = "python")]
fn test_python_sdk_v012_subagents_and_config_parity() {
    use pyo3::types::{PyAnyMethods, PyDictMethods};

    let _guard = PYTHON_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    pyo3::Python::initialize();
    pyo3::Python::attach(|py| {
        init_py_sys_path(py);

        let agy = py
            .import("google.antigravity")
            .expect("import google.antigravity");
        let tp = py
            .import("google.antigravity.types")
            .expect("import google.antigravity.types");

        // Verify SubagentConfig in Python SDK
        assert!(
            tp.hasattr("SubagentConfig")
                .expect("hasattr SubagentConfig")
        );
        py.run(
            c"import google.antigravity.types as tp\ntp.SubagentCapabilities.model_rebuild(_parent_namespace_depth=2)\ntp.SubagentConfig.model_rebuild(_parent_namespace_depth=2)",
            None,
            None,
        ).expect("rebuild models");
        let subagent_cls = tp.getattr("SubagentConfig").expect("get SubagentConfig");
        let sub_kwargs = pyo3::types::PyDict::new(py);
        sub_kwargs.set_item("name", "test_subagent").unwrap();
        sub_kwargs
            .set_item("description", "A test subagent")
            .unwrap();
        let py_sub = subagent_cls
            .call((), Some(&sub_kwargs))
            .expect("create SubagentConfig");
        assert_eq!(
            py_sub.getattr("name").unwrap().extract::<String>().unwrap(),
            "test_subagent"
        );

        let budget_cls = tp.getattr("BudgetConfig").expect("get BudgetConfig");
        let py_budget = budget_cls.call0().expect("create default BudgetConfig");

        // Verify full LocalAgentConfig with budget and subagents
        let loc_cls = agy
            .getattr("LocalAgentConfig")
            .expect("get LocalAgentConfig");
        let cfg_kwargs = pyo3::types::PyDict::new(py);
        let sub_list = pyo3::types::PyList::new(py, &[py_sub]).unwrap();
        cfg_kwargs.set_item("subagents", sub_list).unwrap();
        cfg_kwargs.set_item("budget_config", py_budget).unwrap();
        let py_cfg = loc_cls
            .call((), Some(&cfg_kwargs))
            .expect("create LocalAgentConfig");
        assert!(!py_cfg.getattr("budget_config").unwrap().is_none());
    });
}

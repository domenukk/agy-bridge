//! Unit tests for [`super`] — the Python runtime manager.

use super::{ffi_dispatch::check_tool_execution_allowed, *};

fn test_config() -> RuntimeConfig {
    RuntimeConfig {
        channel_capacity: 16,
        shutdown_timeout: Duration::from_secs(5),
        inter_agent_delay: Duration::from_millis(100),
        backend_log_level: BackendLogLevel::default(),
    }
}

#[tokio::test]
async fn test_runtime_creation_and_shutdown() {
    // Shutdown should complete cleanly.
    PythonRuntime::new(test_config())
        .expect("Failed to create runtime")
        .shutdown()
        .await
        .expect("Shutdown failed");
}

#[test]
fn runtime_config_serde_roundtrip() {
    let config = test_config();
    let json = serde_json::to_string(&config).unwrap();
    let parsed: RuntimeConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.channel_capacity, 16);
    assert_eq!(parsed.shutdown_timeout, Duration::from_secs(5));
    assert_eq!(parsed.inter_agent_delay, Duration::from_millis(100));
    assert_eq!(parsed.backend_log_level, BackendLogLevel::Warn);
}

#[test]
fn backend_log_level_default_is_warn() {
    assert_eq!(BackendLogLevel::default(), BackendLogLevel::Warn);
}

#[test]
fn backend_log_level_serde_roundtrip_all_variants() {
    for (variant, expected_str) in [
        (BackendLogLevel::Error, "\"error\""),
        (BackendLogLevel::Warn, "\"warn\""),
        (BackendLogLevel::Info, "\"info\""),
        (BackendLogLevel::Debug, "\"debug\""),
    ] {
        let json = serde_json::to_string(&variant).unwrap();
        assert_eq!(json, expected_str, "serialize {variant:?}");
        let parsed: BackendLogLevel = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, variant, "roundtrip {variant:?}");
    }
}

#[test]
fn backend_log_level_as_str() {
    assert_eq!(BackendLogLevel::Error.as_str(), "error");
    assert_eq!(BackendLogLevel::Warn.as_str(), "warn");
    assert_eq!(BackendLogLevel::Info.as_str(), "info");
    assert_eq!(BackendLogLevel::Debug.as_str(), "debug");
}

#[test]
fn backend_log_level_display() {
    assert_eq!(format!("{}", BackendLogLevel::Error), "error");
    assert_eq!(format!("{}", BackendLogLevel::Warn), "warn");
    assert_eq!(format!("{}", BackendLogLevel::Info), "info");
    assert_eq!(format!("{}", BackendLogLevel::Debug), "debug");
}

#[test]
fn runtime_config_with_custom_backend_log_level() {
    let config = RuntimeConfig {
        backend_log_level: BackendLogLevel::Debug,
        ..test_config()
    };
    let json = serde_json::to_string(&config).unwrap();
    let parsed: RuntimeConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.backend_log_level, BackendLogLevel::Debug);
}

#[test]
fn stop_candidate_exception_is_backend_error() {
    Python::initialize();
    Python::attach(|py| {
        let globals = pyo3::types::PyDict::new(py);
        py.run(
            c"
class StopCandidateException(Exception):
    pass
err = StopCandidateException(\"dummy\")
",
            Some(&globals),
            None,
        )
        .unwrap();

        let err_obj = globals.get_item("err").unwrap().unwrap();
        let err = PyErr::from_value(err_obj);

        let mapped = crate::error::classify_py_error(py, &err);

        assert!(
            matches!(mapped, crate::error::Error::BackendError { .. }),
            "StopCandidateException should be classified as BackendError, got: {mapped:?}"
        );
    });
}

#[test]
fn max_tokens_exception_is_backend_error() {
    Python::initialize();
    Python::attach(|py| {
        let globals = pyo3::types::PyDict::new(py);
        py.run(
            c"
class MaxTokensException(Exception):
    pass
err = MaxTokensException(\"dummy\")
",
            Some(&globals),
            None,
        )
        .unwrap();

        let err_obj = globals.get_item("err").unwrap().unwrap();
        let err = PyErr::from_value(err_obj);

        let mapped = crate::error::classify_py_error(py, &err);

        assert!(
            matches!(mapped, crate::error::Error::BackendError { .. }),
            "MaxTokensException should be classified as BackendError, got: {mapped:?}"
        );
    });
}

struct MockAskUserHandler {
    should_allow: std::sync::atomic::AtomicBool,
}

impl crate::policies::AskUserHandler for MockAskUserHandler {
    fn confirm(&self, _tool_name: &str, _tool_args: &serde_json::Value) -> bool {
        self.should_allow.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[test]
fn test_ask_user_policy_custom_tool_gating() {
    let agent_id: u64 = 999;

    // 1. Setup the PolicySet with an AskUser rule for "dangerous_tool"
    let mut policies = crate::policies::PolicySet::new();
    policies
        .push(crate::policies::PolicyRule::AskUser {
            tool: "dangerous_tool".to_owned(),
            handler_id: "confirm_handler".to_owned(),
        })
        .unwrap();

    // 2. Setup mock handler
    let handler = Arc::new(MockAskUserHandler {
        should_allow: std::sync::atomic::AtomicBool::new(true),
    });

    // 3. Mock the tool registry
    let mut registry = crate::tools::ToolRegistry::new();

    /// A dangerous tool.
    #[crate::llm_tool]
    fn dangerous_tool() -> Result<String, String> {
        Ok("Executed dangerous action!".to_owned())
    }
    registry.register(DangerousTool);

    // 4. Register all state in a single bridge_state() insertion
    bridge_state().write().unwrap().insert(
        agent_id,
        AgentBridgeState {
            registry: Some(Arc::new(registry)),
            hook_runner: None,
            policies,
            policy_handler: Some(Arc::clone(&handler) as Arc<dyn crate::policies::AskUserHandler>),
            tool_state: llm_tool::SharedState::new(),
            last_tool_error: std::sync::Mutex::new(None),
        },
    );

    // 5. Simulate check_tool_execution_allowed when the AskUserHandler allows it (returns true)
    handler
        .should_allow
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let res = check_tool_execution_allowed(agent_id, "dangerous_tool", "{}");
    assert!(res.is_ok(), "Check should succeed");
    assert!(
        res.unwrap(),
        "Should allow tool execution when handler returns true"
    );

    // 6. Simulate check_tool_execution_allowed when the AskUserHandler denies it (returns false)
    handler
        .should_allow
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let res = check_tool_execution_allowed(agent_id, "dangerous_tool", "{}");
    assert!(res.is_ok(), "Check should succeed");
    assert!(
        !res.unwrap(),
        "Should block tool execution when handler returns false"
    );

    // Clean up
    bridge_state().write().unwrap().remove(&agent_id);
}

// ── compute_active_builtins tests ─────────────────────────────────

#[test]
fn builtins_default_config_returns_all() {
    let config = crate::config::AgentConfig::default();
    let builtins = super::compute_active_builtins(&config);
    assert_eq!(
        builtins.len(),
        crate::config::BuiltinTools::all_tools().len(),
        "default config should produce all builtins"
    );
}

#[test]
fn builtins_no_capabilities_returns_all() {
    let config = crate::config::AgentConfig {
        capabilities: None,
        ..crate::config::AgentConfig::default()
    };
    let builtins = super::compute_active_builtins(&config);
    assert_eq!(
        builtins.len(),
        crate::config::BuiltinTools::all_tools().len(),
    );
}

#[test]
fn builtins_enabled_tools_filters() {
    let config = crate::config::AgentConfig {
        capabilities: Some(crate::config::CapabilitiesConfig {
            enabled_tools: Some(vec![
                crate::config::BuiltinTools::ViewFile,
                crate::config::BuiltinTools::ListDir,
            ]),
            ..crate::config::CapabilitiesConfig::default()
        }),
        ..crate::config::AgentConfig::default()
    };
    let builtins = super::compute_active_builtins(&config);
    assert_eq!(builtins.len(), 2);
    assert!(builtins.contains(&crate::config::BuiltinTools::ViewFile));
    assert!(builtins.contains(&crate::config::BuiltinTools::ListDir));
}

#[test]
fn builtins_disabled_tools_excludes() {
    let config = crate::config::AgentConfig {
        capabilities: Some(crate::config::CapabilitiesConfig {
            disabled_tools: Some(vec![crate::config::BuiltinTools::RunCommand]),
            ..crate::config::CapabilitiesConfig::default()
        }),
        ..crate::config::AgentConfig::default()
    };
    let builtins = super::compute_active_builtins(&config);
    assert!(
        !builtins.contains(&crate::config::BuiltinTools::RunCommand),
        "RunCommand should be excluded"
    );
    assert!(
        builtins.len() == crate::config::BuiltinTools::all_tools().len() - 1,
        "should have all builtins minus the disabled one"
    );
}

#[test]
fn builtins_custom_tools_only_returns_empty() {
    let config = crate::config::AgentConfig {
        capabilities: Some(crate::config::CapabilitiesConfig::custom_tools_only()),
        ..crate::config::AgentConfig::default()
    };
    let builtins = super::compute_active_builtins(&config);
    assert!(
        builtins.is_empty(),
        "custom_tools_only should produce 0 builtins"
    );
}

#[test]
fn builtins_all_descriptions_non_empty() {
    for tool in crate::config::BuiltinTools::all_tools() {
        assert!(
            !tool.description().is_empty(),
            "builtin {tool:?} has empty description",
        );
    }
}

#[test]
fn builtins_all_sdk_names_non_empty() {
    for tool in crate::config::BuiltinTools::all_tools() {
        assert!(
            !tool.as_sdk_name().is_empty(),
            "builtin {tool:?} has empty SDK name",
        );
    }
}

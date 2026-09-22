use std::str::FromStr;

use agy_bridge::{
    config::{
        AgentConfig, CapabilitiesConfig, GenerationConfig, RunCommandConfig,
        SessionContinuationMode, SubagentCapabilities, SubagentConfig,
    },
    hooks::{Hooks, StopArgs, StopDecision, StopHookResult},
    types::{ServiceTier, StopReason, UsageMetadata},
};

#[test]
fn test_run_command_config_defaults_and_builder() {
    let default_cfg = RunCommandConfig::default();
    assert!(!default_cfg.enable_daemons);
    assert_eq!(default_cfg.timeout_seconds, None);
    assert!(!default_cfg.enable_sandbox);

    let custom_cfg = RunCommandConfig::builder()
        .enable_daemons(true)
        .timeout_seconds(45.5)
        .enable_sandbox(true)
        .build();
    assert!(custom_cfg.enable_daemons);
    assert_eq!(custom_cfg.timeout_seconds, Some(45.5));
    assert!(custom_cfg.enable_sandbox);

    let json = serde_json::to_string(&custom_cfg).expect("serialize");
    let deserialized: RunCommandConfig = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(custom_cfg, deserialized);
}

#[test]
fn test_capabilities_config_with_run_command_config() {
    let rcc = RunCommandConfig::builder()
        .enable_daemons(true)
        .timeout_seconds(60.0)
        .enable_sandbox(true)
        .build();

    let cap = CapabilitiesConfig::builder()
        .run_command_config(rcc.clone())
        .build();
    assert_eq!(cap.run_command_config, Some(rcc));

    let json = serde_json::to_string(&cap).expect("serialize");
    let deserialized: CapabilitiesConfig = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(cap, deserialized);
}

#[test]
fn test_subagent_capabilities_with_run_command_config() {
    let rcc = RunCommandConfig::builder()
        .enable_daemons(false)
        .timeout_seconds(15.0)
        .enable_sandbox(true)
        .build();

    let sub_cap = SubagentCapabilities::builder()
        .run_command_config(rcc.clone())
        .build();
    assert_eq!(sub_cap.run_command_config, Some(rcc));

    let sub = SubagentConfig::builder()
        .name("runner")
        .description("Command runner")
        .system_instructions("Execute shell commands")
        .capabilities(sub_cap)
        .build();

    let json = serde_json::to_string(&sub).expect("serialize");
    let deserialized: SubagentConfig = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(sub, deserialized);
}

#[test]
fn test_service_tier_variants_and_serde() {
    assert_eq!(ServiceTier::default(), ServiceTier::Unknown);
    assert_eq!(ServiceTier::Standard.as_str(), "standard");
    assert_eq!(ServiceTier::Priority.as_str(), "priority");
    assert_eq!(ServiceTier::Flex.as_str(), "flex");
    assert_eq!(ServiceTier::Unknown.as_str(), "unknown");

    assert_eq!(
        ServiceTier::from_str("standard").unwrap(),
        ServiceTier::Standard
    );
    assert_eq!(
        ServiceTier::from_str("priority").unwrap(),
        ServiceTier::Priority
    );
    assert_eq!(ServiceTier::from_str("flex").unwrap(), ServiceTier::Flex);
    assert_eq!(
        ServiceTier::from_str("STANDARD").unwrap(),
        ServiceTier::Standard
    );
    assert_eq!(
        ServiceTier::from_str("PRIORITY").unwrap(),
        ServiceTier::Priority
    );
    assert_eq!(ServiceTier::from_str("FLEX").unwrap(), ServiceTier::Flex);

    let json = serde_json::to_string(&ServiceTier::Priority).expect("serialize");
    assert_eq!(json, "\"priority\"");
    let parsed: ServiceTier = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed, ServiceTier::Priority);
}

#[test]
fn test_service_tier_in_usage_metadata_and_generation_config() {
    let usage = UsageMetadata {
        prompt_token_count: Some(100),
        cached_content_token_count: None,
        candidates_token_count: Some(50),
        thoughts_token_count: None,
        total_token_count: Some(150),
        prompt_tokens_details: Vec::new(),
        cache_tokens_details: Vec::new(),
        candidates_tokens_details: Vec::new(),
        tool_use_prompt_tokens_details: Vec::new(),
        service_tier: Some(ServiceTier::Priority),
    };

    let json = serde_json::to_string(&usage).expect("serialize");
    let parsed: UsageMetadata = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed.service_tier, Some(ServiceTier::Priority));

    let gen_cfg = GenerationConfig {
        thinking_level: None,
        service_tier: Some(ServiceTier::Flex),
    };
    let gen_json = serde_json::to_string(&gen_cfg).expect("serialize");
    let parsed_gen: GenerationConfig = serde_json::from_str(&gen_json).expect("deserialize");
    assert_eq!(parsed_gen.service_tier, Some(ServiceTier::Flex));
}

#[test]
fn test_session_continuation_mode_variants_and_proto() {
    assert_eq!(SessionContinuationMode::Resume.as_str(), "resume");
    assert_eq!(
        SessionContinuationMode::CreateOrResume.as_str(),
        "create_or_resume"
    );
    assert_eq!(SessionContinuationMode::CreateOnly.as_str(), "create_only");

    assert_eq!(
        SessionContinuationMode::from_str("resume").unwrap(),
        SessionContinuationMode::Resume
    );
    assert_eq!(
        SessionContinuationMode::from_str("create_or_resume").unwrap(),
        SessionContinuationMode::CreateOrResume
    );
    assert_eq!(
        SessionContinuationMode::from_str("create_only").unwrap(),
        SessionContinuationMode::CreateOnly
    );

    assert_eq!(SessionContinuationMode::Resume.to_proto_i32(), 1);
    assert_eq!(SessionContinuationMode::CreateOrResume.to_proto_i32(), 2);
    assert_eq!(SessionContinuationMode::CreateOnly.to_proto_i32(), 3);

    assert_eq!(
        SessionContinuationMode::from_proto_i32(1),
        Some(SessionContinuationMode::Resume)
    );
    assert_eq!(
        SessionContinuationMode::from_proto_i32(2),
        Some(SessionContinuationMode::CreateOrResume)
    );
    assert_eq!(
        SessionContinuationMode::from_proto_i32(3),
        Some(SessionContinuationMode::CreateOnly)
    );
    assert_eq!(SessionContinuationMode::from_proto_i32(0), None);

    let config = AgentConfig::builder()
        .session_continuation_mode(SessionContinuationMode::CreateOrResume)
        .build();
    assert_eq!(
        config.session_continuation_mode,
        Some(SessionContinuationMode::CreateOrResume)
    );

    let json = serde_json::to_string(&config).expect("serialize");
    let parsed: AgentConfig = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(
        parsed.session_continuation_mode,
        Some(SessionContinuationMode::CreateOrResume)
    );
}

#[test]
fn test_stop_hook_types_and_runner() {
    assert_eq!(StopDecision::default(), StopDecision::AllowStop);
    assert_eq!(StopDecision::AllowStop.as_str(), "ALLOW_STOP");
    assert_eq!(StopDecision::Continue.as_str(), "CONTINUE");

    assert_eq!(StopDecision::AllowStop.to_proto_i32(), 1);
    assert_eq!(StopDecision::Continue.to_proto_i32(), 2);
    assert_eq!(
        StopDecision::from_proto_i32(1),
        Some(StopDecision::AllowStop)
    );
    assert_eq!(
        StopDecision::from_proto_i32(2),
        Some(StopDecision::Continue)
    );
    assert_eq!(StopDecision::from_proto_i32(0), None);

    let args = StopArgs {
        response_text: "Finished task".to_string(),
        trajectory_id: "traj-001".to_string(),
        continuation_count: 1,
        stop_reason: StopReason::MaxModelCallsExceeded,
        error_message: String::new(),
    };
    let args_json = serde_json::to_string(&args).expect("serialize");
    let parsed_args: StopArgs = serde_json::from_str(&args_json).expect("deserialize");
    assert_eq!(args, parsed_args);

    let allow_res = StopHookResult::allow();
    assert_eq!(allow_res.decision, StopDecision::AllowStop);
    assert!(allow_res.reason.is_empty());

    let cont_res = StopHookResult::continue_with("Keep working");
    assert_eq!(cont_res.decision, StopDecision::Continue);
    assert_eq!(cont_res.reason, "Keep working");

    // Test Hooks default run_stop
    let empty_hooks = Hooks::new();
    let default_result = empty_hooks.run_stop(&args);
    assert_eq!(default_result.decision, StopDecision::AllowStop);

    // Test Hooks with custom stop hook
    let custom_hooks = Hooks::new().with_stop("custom_stop", |a| {
        if a.continuation_count < 2 {
            StopHookResult::continue_with("More steps needed")
        } else {
            StopHookResult::allow()
        }
    });

    let res1 = custom_hooks.run_stop(&args);
    assert_eq!(res1.decision, StopDecision::Continue);
    assert_eq!(res1.reason, "More steps needed");

    let mut args2 = args.clone();
    args2.continuation_count = 2;
    let res2 = custom_hooks.run_stop(&args2);
    assert_eq!(res2.decision, StopDecision::AllowStop);
}

#[test]
fn test_target_antigravity_sdk_version_exported() {
    let version = env!("TARGET_ANTIGRAVITY_SDK_VERSION");
    assert_eq!(version, "0.1.17");
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
fn test_python_sdk_v016_run_command_config() {
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

        assert!(types_mod.hasattr("RunCommandConfig").unwrap());
        let rcc_cls = types_mod.getattr("RunCommandConfig").unwrap();
        let rcc_kwargs = pyo3::types::PyDict::new(py);
        rcc_kwargs.set_item("enable_daemons", true).unwrap();
        rcc_kwargs.set_item("timeout_seconds", 30.5).unwrap();
        rcc_kwargs.set_item("enable_sandbox", true).unwrap();
        let rcc_instance = rcc_cls.call((), Some(&rcc_kwargs)).unwrap();

        assert!(
            rcc_instance
                .getattr("enable_daemons")
                .unwrap()
                .extract::<bool>()
                .unwrap()
        );
        let timeout_val = rcc_instance
            .getattr("timeout_seconds")
            .unwrap()
            .extract::<f64>()
            .unwrap();
        assert!((timeout_val - 30.5).abs() < f64::EPSILON);
        assert!(
            rcc_instance
                .getattr("enable_sandbox")
                .unwrap()
                .extract::<bool>()
                .unwrap()
        );
    });
}

#[test]
#[cfg(feature = "python")]
fn test_python_sdk_v016_enums() {
    use pyo3::types::PyAnyMethods;

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

        assert!(types_mod.hasattr("ServiceTier").unwrap());
        let service_tier_cls = types_mod.getattr("ServiceTier").unwrap();
        let standard = service_tier_cls.getattr("STANDARD").unwrap();
        assert_eq!(
            standard
                .getattr("value")
                .unwrap()
                .extract::<String>()
                .unwrap(),
            "standard"
        );
        let priority = service_tier_cls.getattr("PRIORITY").unwrap();
        assert_eq!(
            priority
                .getattr("value")
                .unwrap()
                .extract::<String>()
                .unwrap(),
            "priority"
        );
        let flex = service_tier_cls.getattr("FLEX").unwrap();
        assert_eq!(
            flex.getattr("value").unwrap().extract::<String>().unwrap(),
            "flex"
        );

        assert!(types_mod.hasattr("SessionContinuationMode").unwrap());
        let scm_cls = types_mod.getattr("SessionContinuationMode").unwrap();
        let resume = scm_cls.getattr("RESUME").unwrap();
        assert_eq!(
            resume
                .getattr("value")
                .unwrap()
                .extract::<String>()
                .unwrap(),
            "resume"
        );
        let cor = scm_cls.getattr("CREATE_OR_RESUME").unwrap();
        assert_eq!(
            cor.getattr("value").unwrap().extract::<String>().unwrap(),
            "create_or_resume"
        );
        let co = scm_cls.getattr("CREATE_ONLY").unwrap();
        assert_eq!(
            co.getattr("value").unwrap().extract::<String>().unwrap(),
            "create_only"
        );
    });
}

#[test]
#[cfg(feature = "python")]
fn test_python_sdk_v016_stop_hook() {
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

        assert!(types_mod.hasattr("StopDecision").unwrap());
        let stop_decision_cls = types_mod.getattr("StopDecision").unwrap();
        let allow = stop_decision_cls.getattr("ALLOW_STOP").unwrap();
        assert_eq!(
            allow.getattr("value").unwrap().extract::<String>().unwrap(),
            "ALLOW_STOP"
        );
        let cont = stop_decision_cls.getattr("CONTINUE").unwrap();
        assert_eq!(
            cont.getattr("value").unwrap().extract::<String>().unwrap(),
            "CONTINUE"
        );

        assert!(types_mod.hasattr("StopHookResult").unwrap());
        let shr_cls = types_mod.getattr("StopHookResult").unwrap();
        let shr_kwargs = pyo3::types::PyDict::new(py);
        shr_kwargs.set_item("decision", cont).unwrap();
        shr_kwargs.set_item("reason", "Keep working").unwrap();
        let shr_instance = shr_cls.call((), Some(&shr_kwargs)).unwrap();
        assert_eq!(
            shr_instance
                .getattr("reason")
                .unwrap()
                .extract::<String>()
                .unwrap(),
            "Keep working"
        );
    });
}

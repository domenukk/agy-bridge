//! Regression tests for the 5 adversarial review findings in `agy-bridge 0.16.8`:
//! 1. Standalone `PolicyRule::WorkspaceOnly` permits in-workspace builtin & custom tools
//!    while denying outside-workspace access on both `NativeRuntime` and `PythonRuntime`.
//! 2. `AllowAll` without `WorkspaceOnly` sets `WORKSPACE_CONTAINMENT_DISABLED` on `NativeRuntime`
//!    (unit-tested in `native::config::tests` and verified end-to-end here).
//! 3. `env_var` does not implicitly load `.env` from CWD/ancestors unless `load_dotenv()` was called.
//! 4. Symlink-safe CSPRNG `default_save_dir` and symlink rejection in `NativeRuntime`.
//! 5. `AskUserHandler::confirm` is invoked strictly once per custom tool call (and twice when
//!    `TransformToolInput` mutates `args`) on both `NativeRuntime` and `PythonRuntime`.

use agy_bridge::policies::{PolicyDecision, PolicyRule, PolicySet};

#[cfg(any(feature = "native", feature = "python"))]
mod runtime_review_tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use agy_bridge::{
        config::{AgentConfig, BuiltinTools, CapabilitiesConfig, GeminiConfig, ModelConfig},
        hooks::Hooks,
        policies::{AskUserHandler, PolicyRule, ask_user},
        tools::{ToolError, ToolOutput, ToolRegistry, llm_tool},
    };
    use agy_bridge_test_support::{MockGeminiServer, MockResponse};

    #[llm_tool(description = "Read a short status tag from memory")]
    async fn custom_status_probe(
        /// Short probe label.
        label: String,
    ) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::new(format!("custom-probe-ok:{label}")))
    }

    #[llm_tool(description = "Perform an interactive confirmed operation")]
    async fn confirmed_custom_op(
        /// Target identifier.
        target: String,
    ) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::new(format!("executed:{target}")))
    }

    #[cfg(feature = "native")]
    pub(super) fn register_confirmed_op(reg: &mut ToolRegistry) {
        reg.register(ConfirmedCustomOp);
    }

    #[derive(Clone)]
    pub(super) struct SharedConfirmHandler {
        calls: Arc<AtomicUsize>,
        seen_args: Arc<Mutex<Vec<serde_json::Value>>>,
    }

    impl SharedConfirmHandler {
        pub(super) fn new() -> Self {
            Self {
                calls: Arc::new(AtomicUsize::new(0)),
                seen_args: Arc::new(Mutex::new(Vec::new())),
            }
        }

        pub(super) fn count(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        pub(super) fn recorded_args(&self) -> Vec<serde_json::Value> {
            self.seen_args.lock().expect("seen_args lock").clone()
        }
    }

    impl AskUserHandler for SharedConfirmHandler {
        fn confirm(&self, _tool_name: &str, tool_args: &serde_json::Value) -> bool {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.seen_args
                .lock()
                .expect("seen_args lock")
                .push(tool_args.clone());
            true
        }
    }

    pub(super) async fn verify_standalone_workspace_only_on_bridge<
        R: agy_bridge::agent::Runtime,
    >(
        bridge: &agy_bridge::AgyBridge<R>,
    ) {
        let ws_dir = tempfile::tempdir().expect("create workspace tempdir");
        let target_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
        std::fs::create_dir_all(&target_root).expect("ensure target_root");
        let out_dir = tempfile::tempdir_in(&target_root).expect("create outside tempdir");
        let save_dir = tempfile::tempdir().expect("create save_dir");

        let inside_file = ws_dir.path().join("allowed.txt");
        let outside_file = out_dir.path().join("secret.txt");
        std::fs::write(&inside_file, "INSIDE_WORKSPACE_TOKEN").expect("write inside file");
        std::fs::write(&outside_file, "OUTSIDE_SECRET_TOKEN").expect("write outside file");

        let server = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "view_file".into(),
                args: serde_json::json!({
                    "AbsolutePath": inside_file.to_string_lossy(),
                    "toolSummary": "Read inside file",
                    "toolAction": "Reading inside file"
                }),
            },
            MockResponse::FunctionCall {
                name: "custom_status_probe".into(),
                args: serde_json::json!({
                    "label": "alpha",
                    "toolSummary": "Probe status",
                    "toolAction": "Probing status"
                }),
            },
            MockResponse::FunctionCall {
                name: "view_file".into(),
                args: serde_json::json!({
                    "AbsolutePath": outside_file.to_string_lossy(),
                    "toolSummary": "Read outside file",
                    "toolAction": "Reading outside file"
                }),
            },
            MockResponse::Text("Workspace test finished.".into()),
        ])
        .await;

        let mut reg = ToolRegistry::new();
        reg.register(CustomStatusProbe);

        let cfg = AgentConfig::builder()
            .system_instructions("Test standalone workspace_only policy.")
            .save_dir(save_dir.path())
            .gemini(GeminiConfig {
                api_key: Some("test-key".into()),
                base_url: Some(server.base_url()),
                models: ModelConfig::default(),
                ..Default::default()
            })
            .capabilities(CapabilitiesConfig::with_tools(vec![BuiltinTools::ViewFile]))
            .policies([PolicyRule::workspace_only([ws_dir.path()])])
            .build();

        let agent = bridge
            .agent(cfg)
            .tools(reg)
            .await
            .expect("create standalone workspace_only agent");

        let reply = agent
            .chat_text("Run checks")
            .await
            .expect("chat completes cleanly");
        assert_eq!(reply, "Workspace test finished.");

        let posts = server.recorded_posts().await;
        assert!(
            posts.len() >= 4,
            "Expected 4 model turns (initial + 3 tool responses), got {}",
            posts.len()
        );
        let final_req_body = &posts.last().expect("final request").body;
        assert!(
            final_req_body.contains("INSIDE_WORKSPACE_TOKEN"),
            "In-workspace view_file output must reach model: {final_req_body}"
        );
        assert!(
            final_req_body.contains("custom-probe-ok:alpha"),
            "Custom tool output must reach model under standalone WorkspaceOnly: {final_req_body}"
        );
        assert!(
            !final_req_body.contains("OUTSIDE_SECRET_TOKEN"),
            "Outside-workspace file contents must NEVER be read under WorkspaceOnly: {final_req_body}"
        );

        agent.shutdown().await.expect("shutdown agent");
    }

    pub(super) async fn verify_ask_user_single_confirm_on_bridge<R: agy_bridge::agent::Runtime>(
        bridge: &agy_bridge::AgyBridge<R>,
    ) {
        let server = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "confirmed_custom_op".into(),
                args: serde_json::json!({ "target": "original" }),
            },
            MockResponse::Text("Done unmodified.".into()),
        ])
        .await;

        let save_a = tempfile::tempdir().expect("save_a");
        let mut reg_a = ToolRegistry::new();
        reg_a.register(ConfirmedCustomOp);
        let handler_a = SharedConfirmHandler::new();

        let cfg_a = AgentConfig::builder()
            .system_instructions("Confirm once test.")
            .save_dir(save_a.path())
            .gemini(GeminiConfig {
                api_key: Some("test-key".into()),
                base_url: Some(server.base_url()),
                models: ModelConfig::default(),
                ..Default::default()
            })
            .capabilities(CapabilitiesConfig::custom_tools_only())
            .policies([
                ask_user("confirmed_custom_op", "confirm_custom_op"),
                PolicyRule::DenyAll,
            ])
            .build();

        let agent_a = bridge
            .agent(cfg_a)
            .tools(reg_a)
            .policy_handler(handler_a.clone())
            .await
            .expect("create agent_a");
        let text_a = agent_a.chat_text("go").await.expect("chat_a");
        assert_eq!(text_a, "Done unmodified.");
        assert_eq!(
            handler_a.count(),
            1,
            "AskUserHandler::confirm must be invoked strictly ONCE when args are unmodified"
        );
        agent_a.shutdown().await.expect("shutdown_a");
    }

    pub(super) async fn verify_ask_user_mutated_confirm_on_bridge<R: agy_bridge::agent::Runtime>(
        bridge: &agy_bridge::AgyBridge<R>,
    ) {
        let server = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "confirmed_custom_op".into(),
                args: serde_json::json!({ "target": "before-transform" }),
            },
            MockResponse::Text("Done mutated.".into()),
        ])
        .await;

        let save_b = tempfile::tempdir().expect("save_b");
        let mut reg_b = ToolRegistry::new();
        reg_b.register(ConfirmedCustomOp);
        let handler_b = SharedConfirmHandler::new();
        let hooks_b = Hooks::new().with_transform_tool_input("mutate_target", |ctx| {
            if ctx.tool_name == "confirmed_custom_op" {
                Some(serde_json::json!({ "target": "after-transform" }))
            } else {
                None
            }
        });

        let cfg_b = AgentConfig::builder()
            .system_instructions("Confirm twice on transform test.")
            .save_dir(save_b.path())
            .gemini(GeminiConfig {
                api_key: Some("test-key".into()),
                base_url: Some(server.base_url()),
                models: ModelConfig::default(),
                ..Default::default()
            })
            .capabilities(CapabilitiesConfig::custom_tools_only())
            .policies([
                ask_user("confirmed_custom_op", "confirm_custom_op"),
                PolicyRule::DenyAll,
            ])
            .build();

        let agent_b = bridge
            .agent(cfg_b)
            .tools(reg_b)
            .hooks(hooks_b)
            .policy_handler(handler_b.clone())
            .await
            .expect("create agent_b");
        let text_b = agent_b.chat_text("go").await.expect("chat_b");
        assert_eq!(text_b, "Done mutated.");
        assert_eq!(
            handler_b.count(),
            1,
            "localharness 0.1.24 runs PreTool before PolicyDecisionRequest, so confirm() runs strictly ONCE on already-mutated args"
        );
        let seen = handler_b.recorded_args();
        assert_eq!(
            seen[0].get("target").and_then(serde_json::Value::as_str),
            Some("after-transform"),
            "AskUserHandler::confirm must inspect post-TransformToolInput arguments: {seen:?}"
        );
        agent_b.shutdown().await.expect("shutdown_b");
    }
}

#[test]
fn standalone_workspace_only_policy_set_evaluates_to_allow_while_empty_denies() {
    let empty = PolicySet::new();
    assert_eq!(empty.evaluate("custom_status_probe"), PolicyDecision::Deny);

    let standalone = PolicySet::validated_from(vec![PolicyRule::workspace_only(["/tmp/ws"])])
        .expect("valid standalone WorkspaceOnly");
    assert_eq!(
        standalone.evaluate("custom_status_probe"),
        PolicyDecision::Allow
    );
    assert_eq!(standalone.evaluate("view_file"), PolicyDecision::Allow);

    let restricted = PolicySet::validated_from(vec![
        PolicyRule::workspace_only(["/tmp/ws"]),
        PolicyRule::DenyAll,
    ])
    .expect("valid WorkspaceOnly + DenyAll");
    assert_eq!(
        restricted.evaluate("custom_status_probe"),
        PolicyDecision::Deny
    );
}

#[test]
fn env_var_does_not_implicitly_read_cwd_dotenv_without_load_dotenv() {
    assert!(agy_bridge::env_var("AGY_BRIDGE_UNSET_ADVERSARIAL_SENTINEL_9941").is_none());
}

#[cfg(feature = "native")]
async fn run_mock_reconfirm_session(listener: tokio::net::TcpListener) {
    use agy_bridge::proto;
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::{accept_async, tungstenite::Message};

    let (stream, _) = listener.accept().await.expect("accept");
    let mut ws = accept_async(stream).await.expect("ws");
    let _init = ws.next().await.expect("init").expect("ok");
    let init_resp = proto::localharness::OutputEvent {
        event: Some(
            proto::localharness::output_event::Event::InitializeConversationResponse(
                proto::localharness::InitializeConversationResponse {
                    cascade_id: "reconfirm_1".to_string(),
                    ..proto::localharness::InitializeConversationResponse::default()
                },
            ),
        ),
        ..proto::localharness::OutputEvent::default()
    };
    ws.send(Message::Text(
        serde_json::to_string(&init_resp).expect("json").into(),
    ))
    .await
    .expect("send init");
    let _user = ws.next().await.expect("user").expect("ok");

    // 1. PolicyDecisionRequest arrives BEFORE TransformToolInput on {"target":"orig"}.
    let pdr = proto::localharness::OutputEvent {
        event: Some(
            proto::localharness::output_event::Event::PolicyDecisionRequest(
                proto::localharness::PolicyDecisionRequest {
                    request_id: "pdr-1".to_string(),
                    rule_id: "confirm_custom_op".to_string(),
                    tool_args: Some(proto::localharness::PreToolArgs {
                        tool_name: "confirmed_custom_op".to_string(),
                        arguments_json: r#"{"target":"orig"}"#.to_string(),
                        ..proto::localharness::PreToolArgs::default()
                    }),
                    reason: String::new(),
                },
            ),
        ),
        ..proto::localharness::OutputEvent::default()
    };
    ws.send(Message::Text(
        serde_json::to_string(&pdr).expect("json").into(),
    ))
    .await
    .expect("send pdr");
    let _pdr_resp = ws.next().await.expect("pdr_resp").expect("ok");

    // 2. ToolCall arrives without prior CallHookRequest(PreTool), so run_tool_pre_check
    // runs TransformToolInput ("orig" -> "mutated") and MUST re-invoke confirm()!
    let tc = proto::localharness::OutputEvent {
        event: Some(proto::localharness::output_event::Event::ToolCall(
            proto::localharness::ToolCall {
                id: "tc-1".to_string(),
                name: "confirmed_custom_op".to_string(),
                arguments_json: r#"{"target":"orig"}"#.to_string(),
                ..proto::localharness::ToolCall::default()
            },
        )),
        ..proto::localharness::OutputEvent::default()
    };
    ws.send(Message::Text(
        serde_json::to_string(&tc).expect("json").into(),
    ))
    .await
    .expect("send tc");
    let _tc_resp = ws.next().await.expect("tc_resp").expect("ok");

    let done = proto::localharness::OutputEvent {
        event: Some(
            proto::localharness::output_event::Event::TrajectoryStateUpdate(
                proto::localharness::TrajectoryStateUpdate {
                    state: 2,
                    ..proto::localharness::TrajectoryStateUpdate::default()
                },
            ),
        ),
        ..proto::localharness::OutputEvent::default()
    };
    ws.send(Message::Text(
        serde_json::to_string(&done).expect("json").into(),
    ))
    .await
    .expect("send done");
}

#[cfg(feature = "native")]
#[tokio::test]
async fn native_reconfirms_when_args_mutate_after_policy_decision_request() {
    use agy_bridge::{config::AgentConfig, hooks::Hooks, proto};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let server = tokio::spawn(run_mock_reconfirm_session(listener));

    let mock_dir = tempfile::tempdir().expect("mock_dir");
    #[cfg(windows)]
    let bin_name = "mock_localharness.exe";
    #[cfg(not(windows))]
    let bin_name = "mock_localharness";
    let bin_path = mock_dir.path().join(bin_name);
    let mut out_bytes = Vec::new();
    proto::localharness::OutputConfig {
        port: i32::from(port),
        api_key: "mock-key".to_string(),
    }
    .encode(&mut out_bytes)
    .expect("encode");
    let mut frame = u32::try_from(out_bytes.len())
        .expect("len")
        .to_le_bytes()
        .to_vec();
    frame.extend_from_slice(&out_bytes);
    std::fs::write(mock_dir.path().join("payload.dat"), &frame).expect("write payload");
    std::fs::copy(env!("AGY_BRIDGE_MOCK_HARNESS_BIN"), &bin_path).expect("copy mock bin");

    let bridge = agy_bridge::AgyBridge::native_builder()
        .harness_path(&bin_path)
        .inter_agent_delay(std::time::Duration::ZERO)
        .build_native()
        .expect("build");
    let handler = runtime_review_tests::SharedConfirmHandler::new();
    let hooks = Hooks::new().with_transform_tool_input("mut", |_ctx| {
        Some(serde_json::json!({ "target": "mutated" }))
    });
    let mut reg = agy_bridge::tools::ToolRegistry::new();
    runtime_review_tests::register_confirmed_op(&mut reg);
    let agent = bridge
        .agent(
            AgentConfig::builder()
                .policies([
                    agy_bridge::policies::ask_user("confirmed_custom_op", "confirm_custom_op"),
                    PolicyRule::DenyAll,
                ])
                .build(),
        )
        .tools(reg)
        .hooks(hooks)
        .policy_handler(handler.clone())
        .await
        .expect("agent");

    let _reply = agent.chat_text("go").await.expect("chat");
    assert_eq!(
        handler.count(),
        2,
        "Must re-confirm after post-PDR mutation"
    );
    assert_eq!(
        handler.recorded_args(),
        vec![
            serde_json::json!({"target": "orig"}),
            serde_json::json!({"target": "mutated"}),
        ]
    );
    agent.shutdown().await.expect("shutdown");
    server.await.expect("server");
}

#[cfg(feature = "native")]
#[tokio::test]
async fn native_standalone_workspace_only_and_single_confirm() {
    let bridge = agy_bridge::AgyBridge::native_builder()
        .inter_agent_delay(std::time::Duration::ZERO)
        .build_native()
        .expect("build native bridge");
    runtime_review_tests::verify_standalone_workspace_only_on_bridge(&bridge).await;
    runtime_review_tests::verify_ask_user_single_confirm_on_bridge(&bridge).await;
    runtime_review_tests::verify_ask_user_mutated_confirm_on_bridge(&bridge).await;
}

#[cfg(all(feature = "native", unix))]
#[tokio::test]
async fn native_rejects_symlink_save_dir_without_deleting_target() {
    use agy_bridge::config::{AgentConfig, CapabilitiesConfig, GeminiConfig, ModelConfig};

    let real_target = tempfile::tempdir().expect("real_target");
    let sentinel = real_target.path().join("keep_me.txt");
    std::fs::write(&sentinel, "do-not-delete").expect("write sentinel");

    let link_holder = tempfile::tempdir().expect("link_holder");
    let symlink_save_dir = link_holder.path().join("symlink_save");
    std::os::unix::fs::symlink(real_target.path(), &symlink_save_dir)
        .expect("create save_dir symlink");

    let bridge = agy_bridge::AgyBridge::native_builder()
        .inter_agent_delay(std::time::Duration::ZERO)
        .build_native()
        .expect("build native bridge");

    let cfg = AgentConfig::builder()
        .save_dir(&symlink_save_dir)
        .gemini(GeminiConfig {
            api_key: Some("test-key".into()),
            base_url: Some("http://127.0.0.1:9".into()),
            models: ModelConfig::default(),
            ..Default::default()
        })
        .capabilities(CapabilitiesConfig::custom_tools_only())
        .policies([PolicyRule::DenyAll])
        .build();

    let err = match bridge.agent(cfg).await {
        Ok(_unexpected) => panic!("Symlink save_dir must be rejected"),
        Err(e) => e,
    };
    assert!(
        err.to_string().contains("symlink"),
        "Expected symlink rejection error, got: {err}"
    );
    drop(bridge);
    assert!(
        sentinel.is_file(),
        "Symlink target contents must remain untouched"
    );
}

#[cfg(feature = "python")]
#[tokio::test]
async fn python_standalone_workspace_only_and_single_confirm() {
    let bridge = agy_bridge::AgyBridge::python_builder()
        .inter_agent_delay(std::time::Duration::ZERO)
        .build_python()
        .expect("build python bridge");
    runtime_review_tests::verify_standalone_workspace_only_on_bridge(&bridge).await;
    runtime_review_tests::verify_ask_user_single_confirm_on_bridge(&bridge).await;
    runtime_review_tests::verify_ask_user_mutated_confirm_on_bridge(&bridge).await;
}

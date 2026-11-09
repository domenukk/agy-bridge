use super::*;

async fn run_mock_budget_and_subagents_session(listener: TcpListener) {
    let (stream, _) = listener.accept().await.expect("accept connection");
    let mut ws = accept_async(stream).await.expect("ws handshake");

    let init_msg = ws.next().await.expect("first message").expect("valid msg");
    let init_text = init_msg.to_text().expect("text msg");
    let init_event: proto::localharness::InitializeConversationEvent =
        serde_json::from_str(init_text).expect("parse init event");

    let config = init_event.config.expect("config present");
    assert!(config.budget_config.is_some(), "expected budget_config");
    let budget = config.budget_config.unwrap();
    assert_eq!(budget.max_model_calls, 10);
    assert_eq!(budget.max_total_tokens, 50_000);
    assert_eq!(config.agent_behavior, 1); // AUTONOMOUS
    assert_eq!(config.custom_subagents.len(), 1);
    assert_eq!(config.custom_subagents[0].name, "researcher");

    let init_resp = proto::localharness::OutputEvent {
        event: Some(
            proto::localharness::output_event::Event::InitializeConversationResponse(
                proto::localharness::InitializeConversationResponse {
                    cascade_id: "native-cascade-budget".to_string(),
                    ..Default::default()
                },
            ),
        ),
        ..Default::default()
    };
    ws.send(Message::Text(
        serde_json::to_string(&init_resp).unwrap().into(),
    ))
    .await
    .expect("send init resp");

    let user_msg = ws.next().await.expect("user msg").expect("valid msg");
    let user_text = user_msg.to_text().expect("text msg");
    let _input_event: proto::localharness::InputEvent =
        serde_json::from_str(user_text).expect("parse input event");

    let step_event = proto::localharness::OutputEvent {
        event: Some(proto::localharness::output_event::Event::StepUpdate(
            Box::new(proto::localharness::StepUpdate {
                text_delta: "Budget-constrained answer".to_string(),
                text: "Budget-constrained answer".to_string(),
                parent_trajectory_id: "native-cascade-budget".to_string(),
                ..Default::default()
            }),
        )),
        ..Default::default()
    };
    ws.send(Message::Text(
        serde_json::to_string(&step_event).unwrap().into(),
    ))
    .await
    .expect("send step update");

    let usage_event = proto::localharness::OutputEvent {
        event: Some(proto::localharness::output_event::Event::UsageUpdate(
            proto::localharness::UsageUpdate {
                total: Some(proto::localharness::UsageMetadata {
                    prompt_token_count: 100,
                    candidates_token_count: 50,
                    total_token_count: 150,
                    prompt_tokens_details: vec![proto::localharness::ModalityTokenCount {
                        modality: 1, // TEXT
                        token_count: 100,
                    }],
                    ..Default::default()
                }),
                ..Default::default()
            },
        )),
        ..Default::default()
    };
    ws.send(Message::Text(
        serde_json::to_string(&usage_event).unwrap().into(),
    ))
    .await
    .expect("send usage update");

    let state_event = proto::localharness::OutputEvent {
        event: Some(
            proto::localharness::output_event::Event::TrajectoryStateUpdate(
                proto::localharness::TrajectoryStateUpdate {
                    state: 3,
                    stop_reason: 1, // MAX_MODEL_CALLS_EXCEEDED
                    ..Default::default()
                },
            ),
        ),
        ..Default::default()
    };
    ws.send(Message::Text(
        serde_json::to_string(&state_event).unwrap().into(),
    ))
    .await
    .expect("send state update");
}

#[tokio::test]
async fn test_native_backend_budget_and_subagents_e2e() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let port = listener.local_addr().expect("local addr").port();
    let server_task = tokio::spawn(run_mock_budget_and_subagents_session(listener));

    let mock = MockHarness::create(port, "bs");

    let bridge = AgyBridge::native_builder()
        .harness_path(mock.bin_path())
        .build_native()
        .expect("build bridge");

    let config = AgentConfig::builder()
        .budget_config(
            agy_bridge::config::BudgetConfig::builder()
                .max_model_calls(10)
                .max_total_tokens(50_000)
                .build(),
        )
        .capabilities(
            agy_bridge::config::CapabilitiesConfig::builder()
                .agent_behavior(agy_bridge::config::AgentBehavior::Autonomous)
                .max_subagent_depth(2)
                .allowed_subagents(vec!["researcher".to_string()])
                .build(),
        )
        .subagents(vec![
            agy_bridge::config::SubagentConfig::builder()
                .name("researcher")
                .description("Research subagent")
                .build(),
        ])
        .build();

    let agent = bridge.agent(config).await.expect("create agent");

    let reply = agent.chat_text("Perform budget task").await.expect("chat");
    assert_eq!(reply, "Budget-constrained answer");

    agent.shutdown().await.expect("shutdown");
    server_task.await.expect("server task completed");
}

async fn handle_mock_stop_hook_exchange(
    ws: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
) {
    let hook_req_event = proto::localharness::OutputEvent {
        event: Some(proto::localharness::output_event::Event::CallHookRequest(
            proto::localharness::CallHookRequest {
                request_id: "stop-req-1".to_string(),
                name: "stop".to_string(),
                r#type: 9,
                client_id: String::new(),
                args: Some(proto::localharness::call_hook_request::Args::Stop(
                    proto::localharness::StopArgs {
                        response_text: "Checking final work".to_string(),
                        trajectory_id: "traj-stop-1".to_string(),
                        continuation_count: 0,
                        stop_reason: "MAX_MODEL_CALLS_EXCEEDED".to_string(),
                        error_message: String::new(),
                    },
                )),
            },
        )),
        ..Default::default()
    };
    ws.send(Message::Text(
        serde_json::to_string(&hook_req_event).unwrap().into(),
    ))
    .await
    .expect("send stop hook req");

    let hook_resp_msg = ws.next().await.expect("hook resp msg").expect("valid msg");
    let hook_resp_event: proto::localharness::InputEvent =
        serde_json::from_str(hook_resp_msg.to_text().unwrap()).expect("parse hook resp");
    if let Some(proto::localharness::input_event::Event::CallHookResponse(resp)) =
        hook_resp_event.event
    {
        assert_eq!(resp.request_id, "stop-req-1");
        if let Some(proto::localharness::call_hook_response::Result::Stop(stop_res)) = resp.result {
            assert_eq!(stop_res.decision, 2); // CONTINUE
            assert_eq!(stop_res.reason, "Please continue verification");
        } else {
            panic!("Expected StopResult, got {:?}", resp.result);
        }
    } else {
        panic!("Expected CallHookResponse, got {hook_resp_event:?}");
    }
}

async fn run_mock_v016_session(listener: TcpListener) {
    let (stream, _) = listener.accept().await.expect("accept connection");
    let mut ws = accept_async(stream).await.expect("ws handshake");

    let init_msg = ws.next().await.expect("first message").expect("valid msg");
    let _init_event: proto::localharness::InitializeConversationEvent =
        serde_json::from_str(init_msg.to_text().unwrap()).expect("parse init event");

    let init_resp = proto::localharness::OutputEvent {
        event: Some(
            proto::localharness::output_event::Event::InitializeConversationResponse(
                proto::localharness::InitializeConversationResponse {
                    cascade_id: "v016-cascade-1".to_string(),
                    ..Default::default()
                },
            ),
        ),
        ..Default::default()
    };
    ws.send(Message::Text(
        serde_json::to_string(&init_resp).unwrap().into(),
    ))
    .await
    .expect("send init resp");

    let _user_msg = ws.next().await.expect("user msg").expect("valid msg");

    handle_mock_stop_hook_exchange(&mut ws).await;

    let step_event = proto::localharness::OutputEvent {
        event: Some(proto::localharness::output_event::Event::StepUpdate(
            Box::new(proto::localharness::StepUpdate {
                text_delta: "v016 answer".to_string(),
                text: "v016 answer".to_string(),
                ..Default::default()
            }),
        )),
        ..Default::default()
    };
    ws.send(Message::Text(
        serde_json::to_string(&step_event).unwrap().into(),
    ))
    .await
    .expect("send step");

    let usage_event = proto::localharness::OutputEvent {
        event: Some(proto::localharness::output_event::Event::UsageUpdate(
            proto::localharness::UsageUpdate {
                total: Some(proto::localharness::UsageMetadata {
                    prompt_token_count: 50,
                    candidates_token_count: 25,
                    total_token_count: 75,
                    service_tier: "priority".to_string(),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )),
        ..Default::default()
    };
    ws.send(Message::Text(
        serde_json::to_string(&usage_event).unwrap().into(),
    ))
    .await
    .expect("send usage update");

    let state_event = proto::localharness::OutputEvent {
        event: Some(
            proto::localharness::output_event::Event::TrajectoryStateUpdate(
                proto::localharness::TrajectoryStateUpdate {
                    state: 3,
                    ..Default::default()
                },
            ),
        ),
        ..Default::default()
    };
    ws.send(Message::Text(
        serde_json::to_string(&state_event).unwrap().into(),
    ))
    .await
    .expect("send state update");
}

#[tokio::test]
async fn test_native_backend_v016_features_e2e() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let port = listener.local_addr().expect("local addr").port();
    let server_task = tokio::spawn(run_mock_v016_session(listener));

    let mock = MockHarness::create(port, "v016");

    let bridge = AgyBridge::native_builder()
        .harness_path(mock.bin_path())
        .build_native()
        .expect("build bridge");

    let stop_called = Arc::new(AtomicBool::new(false));
    let stop_flag = stop_called.clone();

    let mut hooks = agy_bridge::hooks::Hooks::new();
    hooks.on_stop("stop_checker", move |args| {
        assert_eq!(args.continuation_count, 0);
        assert_eq!(
            args.stop_reason,
            agy_bridge::types::StopReason::MaxModelCallsExceeded
        );
        assert_eq!(args.response_text, "Checking final work");
        assert_eq!(args.trajectory_id, "traj-stop-1");
        stop_flag.store(true, Ordering::SeqCst);
        agy_bridge::hooks::StopHookResult::continue_with("Please continue verification")
    });

    let config = AgentConfig::builder()
        .session_continuation_mode(agy_bridge::config::SessionContinuationMode::CreateOrResume)
        .capabilities(
            agy_bridge::config::CapabilitiesConfig::builder()
                .run_command_config(
                    agy_bridge::config::RunCommandConfig::builder()
                        .enable_daemons(true)
                        .timeout_seconds(30.0)
                        .enable_sandbox(true)
                        .build(),
                )
                .build(),
        )
        .build();

    let agent = bridge
        .agent(config)
        .hooks(hooks)
        .await
        .expect("create agent");

    let reply = agent.chat_text("Perform v016 task").await.expect("chat");
    assert_eq!(reply, "v016 answer");
    assert!(stop_called.load(Ordering::SeqCst));

    agent.shutdown().await.expect("shutdown");
    server_task.await.expect("server task completed");
}

async fn run_mock_v017_session(listener: TcpListener) {
    let (stream, _) = listener.accept().await.expect("accept");
    let mut ws = tokio_tungstenite::accept_async(stream)
        .await
        .expect("ws accept");

    let _init_msg = ws.next().await.expect("init msg").expect("valid msg");

    let init_resp = proto::localharness::OutputEvent {
        event: Some(
            proto::localharness::output_event::Event::InitializeConversationResponse(
                proto::localharness::InitializeConversationResponse {
                    cascade_id: "traj-v017-mock".to_string(),
                    sandbox_status: Some(proto::localharness::SandboxStatus {
                        available: true,
                        unavailable_reason: String::new(),
                    }),
                    ..Default::default()
                },
            ),
        ),
        ..Default::default()
    };
    ws.send(Message::Text(
        serde_json::to_string(&init_resp).unwrap().into(),
    ))
    .await
    .expect("send init resp");

    let _user_msg = ws.next().await.expect("user msg").expect("valid msg");

    let text_event = proto::localharness::OutputEvent {
        event: Some(proto::localharness::output_event::Event::StepUpdate(
            Box::new(proto::localharness::StepUpdate {
                text_delta: "v017 answer".to_string(),
                text: "v017 answer".to_string(),
                ..Default::default()
            }),
        )),
        ..Default::default()
    };
    ws.send(Message::Text(
        serde_json::to_string(&text_event).unwrap().into(),
    ))
    .await
    .expect("send text");

    let state_event = proto::localharness::OutputEvent {
        event: Some(
            proto::localharness::output_event::Event::TrajectoryStateUpdate(
                proto::localharness::TrajectoryStateUpdate {
                    state: 3,
                    ..Default::default()
                },
            ),
        ),
        ..Default::default()
    };
    ws.send(Message::Text(
        serde_json::to_string(&state_event).unwrap().into(),
    ))
    .await
    .expect("send state update");
}

#[tokio::test]
async fn test_native_backend_v017_features_e2e() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let port = listener.local_addr().expect("local addr").port();
    let server_task = tokio::spawn(run_mock_v017_session(listener));

    let mock = MockHarness::create(port, "v017");

    let bridge = AgyBridge::native_builder()
        .harness_path(mock.bin_path())
        .build_native()
        .expect("build bridge");

    let config = AgentConfig::builder()
        .compaction_config(
            agy_bridge::config::CompactionConfig::builder()
                .token_threshold(50_000)
                .build(),
        )
        .budget_config(
            agy_bridge::config::BudgetConfig::builder()
                .scope(agy_bridge::config::BudgetScope::ForwardLooking)
                .build(),
        )
        .capabilities(
            agy_bridge::config::CapabilitiesConfig::builder()
                .tool_output_truncation_config(
                    agy_bridge::config::ToolOutputTruncationConfig::builder()
                        .max_tokens(1024)
                        .build(),
                )
                .build(),
        )
        .build();

    let agent = bridge.agent(config).await.expect("create agent");

    let sandbox_status = agent.sandbox_status().await.expect("sandbox_status");
    assert_eq!(
        sandbox_status,
        Some(agy_bridge::types::SandboxStatus {
            available: true,
            unavailable_reason: None,
        })
    );

    let reply = agent.chat_text("Perform v017 task").await.expect("chat");
    assert_eq!(reply, "v017 answer");

    agent.shutdown().await.expect("shutdown");
    server_task.await.expect("server task completed");
}

async fn run_mock_v0_1_22_session(listener: TcpListener) {
    let (stream, _) = listener.accept().await.expect("accept");
    let mut ws = tokio_tungstenite::accept_async(stream)
        .await
        .expect("ws accept");

    let init_msg = ws.next().await.expect("init msg").expect("valid msg");
    let init_txt = init_msg.into_text().expect("text");
    assert!(
        init_txt.contains("inline_sec_skill"),
        "Expected inline skill in InitializeConversationRequest: {init_txt}"
    );
    assert!(
        init_txt.contains("Keep crash addresses"),
        "Expected summaryPromptOverride in InitializeConversationRequest: {init_txt}"
    );

    let init_resp = proto::localharness::OutputEvent {
        event: Some(
            proto::localharness::output_event::Event::InitializeConversationResponse(
                proto::localharness::InitializeConversationResponse {
                    cascade_id: "traj-v0122-mock".to_string(),
                    ..Default::default()
                },
            ),
        ),
        ..Default::default()
    };
    ws.send(Message::Text(
        serde_json::to_string(&init_resp).unwrap().into(),
    ))
    .await
    .expect("send init resp");

    let _user_msg = ws.next().await.expect("user msg").expect("valid msg");

    let wf_step_event = proto::localharness::OutputEvent {
        event: Some(proto::localharness::output_event::Event::StepUpdate(
            Box::new(proto::localharness::StepUpdate {
                step_index: 1,
                trajectory_id: "traj-v0122-mock".to_string(),
                cascade_id: "traj-v0122-mock".to_string(),
                state: 3,
                run_workflow: Some(proto::localharness::ActionRunWorkflow {
                    script_path: "/tmp/audit_wf.py".to_string(),
                    script: "phase('triage')".to_string(),
                    description: "Run triage workflow".to_string(),
                    output: "Triage phase complete".to_string(),
                }),
                skill_lookup: Some(proto::localharness::ActionSkillLookup {
                    skill_name: "inline_sec_skill".to_string(),
                }),
                text_delta: "v0.1.22 workflow done".to_string(),
                text: "v0.1.22 workflow done".to_string(),
                ..Default::default()
            }),
        )),
        ..Default::default()
    };
    ws.send(Message::Text(
        serde_json::to_string(&wf_step_event).unwrap().into(),
    ))
    .await
    .expect("send wf step");

    let state_event = proto::localharness::OutputEvent {
        event: Some(
            proto::localharness::output_event::Event::TrajectoryStateUpdate(
                proto::localharness::TrajectoryStateUpdate {
                    state: 3,
                    ..Default::default()
                },
            ),
        ),
        ..Default::default()
    };
    ws.send(Message::Text(
        serde_json::to_string(&state_event).unwrap().into(),
    ))
    .await
    .expect("send state update");
}

#[tokio::test]
async fn test_native_backend_v0_1_22_features_e2e() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let port = listener.local_addr().expect("local addr").port();
    let server_task = tokio::spawn(run_mock_v0_1_22_session(listener));

    let mock = MockHarness::create(port, "v0122");

    let bridge = AgyBridge::native_builder()
        .harness_path(mock.bin_path())
        .build_native()
        .expect("build bridge");

    let inline_skill = agy_bridge::InlineSkill::builder()
        .name("inline_sec_skill")
        .description("Inline security skill")
        .content("# Skill\nCheck bounds.")
        .build();

    let config = AgentConfig::builder()
        .compaction_config(
            agy_bridge::CompactionConfig::one_million()
                .with_summary_prompt_override("Keep crash addresses")
                .with_use_model_reported_token_count(true),
        )
        .capabilities(
            agy_bridge::CapabilitiesConfig::builder()
                .mcp_max_output_bytes(65_536)
                .tool_output_truncation_config(
                    agy_bridge::ToolOutputTruncationConfig::new(4096).with_save_full_output(true),
                )
                .build(),
        )
        .build();

    let agent = bridge
        .agent(config)
        .inline_skills(vec![inline_skill])
        .await
        .expect("create agent");

    let mut handle = agent
        .chat("Run v0.1.22 workflow")
        .await
        .expect("start chat");
    let mut step_rx = handle.take_step_stream().expect("step stream");
    let result = handle.text().await.expect("chat result");
    let mut saw_wf_progress = false;
    let mut saw_skill_lookup = false;
    while let Some(step) = step_rx.recv().await {
        if let Some(ref wf) = step.workflow_progress {
            assert_eq!(wf.script_path, "/tmp/audit_wf.py");
            assert_eq!(wf.output, "Triage phase complete");
            saw_wf_progress = true;
        }
        if step.tool_calls.iter().any(|tc| tc.name == "skill_lookup") {
            saw_skill_lookup = true;
        }
    }
    assert_eq!(result.text(), "v0.1.22 workflow done");
    assert!(saw_wf_progress, "Expected Step.workflow_progress");
    assert!(saw_skill_lookup, "Expected skill_lookup tool call in Step");

    agent.shutdown().await.expect("shutdown");
    server_task.await.expect("server task completed");
}

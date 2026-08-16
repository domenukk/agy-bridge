//! Integration tests for the Native backend (local harness over stdio + WebSocket + Protobuf).

#![cfg(feature = "native")]

use std::{
    fs::{self, Permissions},
    os::unix::fs::PermissionsExt,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use agy_bridge::{
    AgyBridge,
    config::AgentConfig,
    proto,
    runtime::{BackendLogLevel, NativeRuntime, RuntimeConfig},
};
use futures::{SinkExt, StreamExt};
use prost::Message as _;
use tokio::net::TcpListener;
use tokio_tungstenite::{accept_async, tungstenite::Message};

fn create_mock_harness_binary(port: u16, prefix: &str) -> String {
    let output_config = proto::localharness::OutputConfig {
        port: i32::from(port),
        api_key: "mock-api-key".to_string(),
    };
    let mut out_bytes = Vec::new();
    output_config
        .encode(&mut out_bytes)
        .expect("encode output config");

    let mut frame = Vec::new();
    let len_u32 = u32::try_from(out_bytes.len()).expect("len fits u32");
    frame.extend_from_slice(&len_u32.to_le_bytes());
    frame.extend_from_slice(&out_bytes);

    let mock_bin_path = format!("/tmp/mock_localharness_{prefix}_{port}");
    let hex_bytes: Vec<String> = frame.iter().map(|b| format!("\\x{b:02x}")).collect();
    let script_content = format!(
        "#!/bin/sh\nprintf '{}'\nexec sleep 30\n",
        hex_bytes.join("")
    );

    fs::write(&mock_bin_path, script_content).expect("write mock binary");
    fs::set_permissions(&mock_bin_path, Permissions::from_mode(0o755)).expect("set executable");
    mock_bin_path
}

struct MockPolicyHandler;

impl agy_bridge::policies::AskUserHandler for MockPolicyHandler {
    fn confirm(&self, tool_name: &str, _args: &serde_json::Value) -> bool {
        tool_name == "deploy_production"
    }
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct CalcParams {
    a: i64,
    b: i64,
}

struct CalcTool;

impl agy_bridge::tools::RustTool for CalcTool {
    type Params = CalcParams;
    const NAME: &'static str = "calculate_sum";
    const DESCRIPTION: &'static str = "Sums two numbers";

    async fn call(
        &self,
        params: Self::Params,
        _ctx: &agy_bridge::tools::ToolContext,
    ) -> Result<agy_bridge::tools::ToolOutput, agy_bridge::tools::ToolError> {
        Ok(agy_bridge::tools::ToolOutput::new(
            (params.a + params.b).to_string(),
        ))
    }
}

#[test]
fn test_native_bridge_builder_and_config() {
    let bridge = AgyBridge::native_builder()
        .backend_log_level(BackendLogLevel::Debug)
        .inter_agent_delay(Duration::from_millis(50))
        .harness_path("/custom/path/to/localharness")
        .build_native()
        .expect("build_native");

    let cfg = bridge.runtime().config();
    assert_eq!(cfg.backend_log_level, BackendLogLevel::Debug);
    assert_eq!(cfg.inter_agent_delay, Duration::from_millis(50));
    assert_eq!(
        cfg.harness_binary_path,
        Some(std::path::PathBuf::from("/custom/path/to/localharness"))
    );
}

#[tokio::test]
async fn test_native_runtime_agent_count() {
    let runtime = Arc::new(NativeRuntime::new(RuntimeConfig::default()));
    let bridge = AgyBridge::new(runtime);

    let count = bridge
        .active_agent_count()
        .await
        .expect("active_agent_count");
    assert_eq!(count, 0);
}

async fn run_mock_chat_session(listener: TcpListener) {
    let (stream, _) = listener.accept().await.expect("accept connection");
    let mut ws = accept_async(stream).await.expect("ws handshake");

    let init_msg = ws.next().await.expect("first message").expect("valid msg");
    let init_text = init_msg.to_text().expect("text msg");
    let _init_event: proto::localharness::InitializeConversationEvent =
        serde_json::from_str(init_text).expect("parse init event");

    let init_resp = proto::localharness::OutputEvent {
        event: Some(
            proto::localharness::output_event::Event::InitializeConversationResponse(
                proto::localharness::InitializeConversationResponse {
                    cascade_id: "native-cascade-42".to_string(),
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
            proto::localharness::StepUpdate {
                text_delta: "Hello from native backend!".to_string(),
                text: "Hello from native backend!".to_string(),
                ..Default::default()
            },
        )),
        ..Default::default()
    };
    ws.send(Message::Text(
        serde_json::to_string(&step_event).unwrap().into(),
    ))
    .await
    .expect("send step update");

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
async fn test_native_backend_mock_harness_e2e() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let port = listener.local_addr().expect("local addr").port();
    let server_task = tokio::spawn(run_mock_chat_session(listener));

    let mock_bin_path = create_mock_harness_binary(port, "basic");

    let bridge = AgyBridge::native_builder()
        .harness_path(&mock_bin_path)
        .build_native()
        .expect("build bridge");

    let agent = bridge
        .agent(AgentConfig::default())
        .await
        .expect("create agent");

    assert_eq!(
        agent.conversation_id(),
        Some("native-cascade-42".to_string())
    );

    let reply = agent.chat_text("Hello").await.expect("chat text");
    assert_eq!(reply, "Hello from native backend!");

    assert_eq!(
        agent.conversation_id(),
        Some("native-cascade-42".to_string())
    );

    agent.shutdown().await.expect("shutdown agent");
    server_task.await.expect("server task completed");

    // NOLINT: cleanup in test may fail if mock binary was already cleaned up
    let _ = fs::remove_file(&mock_bin_path);
}

async fn run_mock_tool_session(listener: TcpListener) {
    let (stream, _) = listener.accept().await.expect("accept connection");
    let mut ws = accept_async(stream).await.expect("ws handshake");

    let init_msg = ws.next().await.expect("first message").expect("valid msg");
    let _init_event: proto::localharness::InitializeConversationEvent =
        serde_json::from_str(init_msg.to_text().unwrap()).expect("parse init event");

    let init_resp = proto::localharness::OutputEvent {
        event: Some(
            proto::localharness::output_event::Event::InitializeConversationResponse(
                proto::localharness::InitializeConversationResponse {
                    cascade_id: "tool-cascade-1".to_string(),
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

    let tool_call_event = proto::localharness::OutputEvent {
        event: Some(proto::localharness::output_event::Event::ToolCall(
            proto::localharness::ToolCall {
                id: "call-calc-1".to_string(),
                name: "calculate_sum".to_string(),
                arguments_json: r#"{"a": 20, "b": 22}"#.to_string(),
                arguments: None,
            },
        )),
        ..Default::default()
    };
    ws.send(Message::Text(
        serde_json::to_string(&tool_call_event).unwrap().into(),
    ))
    .await
    .expect("send tool call");

    let resp_msg = ws
        .next()
        .await
        .expect("tool response msg")
        .expect("valid msg");
    let input_resp: proto::localharness::InputEvent =
        serde_json::from_str(resp_msg.to_text().unwrap()).expect("parse input event");
    if let Some(proto::localharness::input_event::Event::ToolResponse(resp)) = input_resp.event {
        assert_eq!(resp.id, "call-calc-1");
        assert!(resp.response_json.contains("42"));
        assert!(resp.error_message.is_empty());
    } else {
        panic!("Expected ToolResponse event, got {input_resp:?}");
    }

    let final_step_event = proto::localharness::OutputEvent {
        event: Some(proto::localharness::output_event::Event::StepUpdate(
            proto::localharness::StepUpdate {
                text_delta: "The sum is 42.".to_string(),
                text: "The sum is 42.".to_string(),
                ..Default::default()
            },
        )),
        ..Default::default()
    };
    ws.send(Message::Text(
        serde_json::to_string(&final_step_event).unwrap().into(),
    ))
    .await
    .expect("send final step");

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
async fn test_native_backend_custom_tool_dispatch_e2e() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let port = listener.local_addr().expect("local addr").port();
    let server_task = tokio::spawn(run_mock_tool_session(listener));

    let mock_bin_path = create_mock_harness_binary(port, "tool");

    let mut registry = agy_bridge::tools::ToolRegistry::new();
    registry.register(CalcTool);

    let bridge = AgyBridge::native_builder()
        .harness_path(&mock_bin_path)
        .build_native()
        .expect("build bridge");

    let agent = bridge
        .agent(AgentConfig::default())
        .tools(registry)
        .await
        .expect("create agent");

    let reply = agent.chat_text("Add 20 and 22").await.expect("chat");
    assert_eq!(reply, "The sum is 42.");

    agent.shutdown().await.expect("shutdown");
    server_task.await.expect("server task completed");

    // NOLINT: cleanup in test may fail if mock binary was already cleaned up
    let _ = fs::remove_file(&mock_bin_path);
}

async fn handle_mock_hook_exchange(
    ws: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
) {
    let hook_req_event = proto::localharness::OutputEvent {
        event: Some(proto::localharness::output_event::Event::CallHookRequest(
            proto::localharness::CallHookRequest {
                request_id: "hook-req-1".to_string(),
                name: "pre_turn".to_string(),
                r#type: 3,
                args: Some(proto::localharness::call_hook_request::Args::PreTurnArgs(
                    proto::localharness::PreTurnArgs {
                        user_input: Some(proto::localharness::UserInput {
                            parts: vec![proto::localharness::user_input::Part {
                                part: Some(proto::localharness::user_input::part::Part::Text(
                                    "Run sensitive action".to_string(),
                                )),
                            }],
                        }),
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
    .expect("send hook req");

    let hook_resp_msg = ws.next().await.expect("hook resp msg").expect("valid msg");
    let hook_resp_event: proto::localharness::InputEvent =
        serde_json::from_str(hook_resp_msg.to_text().unwrap()).expect("parse hook resp");
    if let Some(proto::localharness::input_event::Event::CallHookResponse(resp)) =
        hook_resp_event.event
    {
        assert_eq!(resp.request_id, "hook-req-1");
        assert!(matches!(
            resp.result,
            Some(proto::localharness::call_hook_response::Result::PreTurnResult(_))
        ));
    } else {
        panic!("Expected CallHookResponse, got {hook_resp_event:?}");
    }
}

async fn handle_mock_policy_exchange(
    ws: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
) {
    let policy_req_event = proto::localharness::OutputEvent {
        event: Some(
            proto::localharness::output_event::Event::PolicyDecisionRequest(
                proto::localharness::PolicyDecisionRequest {
                    request_id: "policy-req-1".to_string(),
                    tool_args: Some(proto::localharness::PreToolArgs {
                        tool_name: "deploy_production".to_string(),
                        arguments_json: r#"{"service":"api"}"#.to_string(),
                        ..Default::default()
                    }),
                    rule_id: "rule-1".to_string(),
                },
            ),
        ),
        ..Default::default()
    };
    ws.send(Message::Text(
        serde_json::to_string(&policy_req_event).unwrap().into(),
    ))
    .await
    .expect("send policy req");

    let policy_resp_msg = ws
        .next()
        .await
        .expect("policy resp msg")
        .expect("valid msg");
    let policy_resp_event: proto::localharness::InputEvent =
        serde_json::from_str(policy_resp_msg.to_text().unwrap()).expect("parse policy resp");
    if let Some(proto::localharness::input_event::Event::PolicyDecisionResponse(resp)) =
        policy_resp_event.event
    {
        assert_eq!(
            resp.outcome,
            proto::localharness::PolicyEvaluationOutcome::Allow as i32
        );
    } else {
        panic!("Expected PolicyDecisionResponse, got {policy_resp_event:?}");
    }
}

async fn run_mock_hooks_and_policy_session(listener: TcpListener) {
    let (stream, _) = listener.accept().await.expect("accept connection");
    let mut ws = accept_async(stream).await.expect("ws handshake");

    let init_msg = ws.next().await.expect("first message").expect("valid msg");
    let _init_event: proto::localharness::InitializeConversationEvent =
        serde_json::from_str(init_msg.to_text().unwrap()).expect("parse init event");

    let init_resp = proto::localharness::OutputEvent {
        event: Some(
            proto::localharness::output_event::Event::InitializeConversationResponse(
                proto::localharness::InitializeConversationResponse {
                    cascade_id: "hook-cascade-1".to_string(),
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

    handle_mock_hook_exchange(&mut ws).await;
    handle_mock_policy_exchange(&mut ws).await;

    let step_event = proto::localharness::OutputEvent {
        event: Some(proto::localharness::output_event::Event::StepUpdate(
            proto::localharness::StepUpdate {
                text_delta: "Action authorized and completed.".to_string(),
                text: "Action authorized and completed.".to_string(),
                ..Default::default()
            },
        )),
        ..Default::default()
    };
    ws.send(Message::Text(
        serde_json::to_string(&step_event).unwrap().into(),
    ))
    .await
    .expect("send step");

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
async fn test_native_backend_hooks_and_policy_e2e() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let port = listener.local_addr().expect("local addr").port();
    let server_task = tokio::spawn(run_mock_hooks_and_policy_session(listener));

    let mock_bin_path = create_mock_harness_binary(port, "hp");

    let pre_turn_called = Arc::new(AtomicBool::new(false));
    let pre_turn_flag = pre_turn_called.clone();

    let mut hooks = agy_bridge::hooks::Hooks::new();
    hooks.on_pre_turn(
        "test_pre_turn",
        move |_ctx: &agy_bridge::hooks::PreTurnContext| {
            pre_turn_flag.store(true, Ordering::SeqCst);
            agy_bridge::hooks::HookResult::allow()
        },
    );

    let bridge = AgyBridge::native_builder()
        .harness_path(&mock_bin_path)
        .build_native()
        .expect("build bridge");

    let agent = bridge
        .agent(AgentConfig::default())
        .hooks(hooks)
        .policy_handler(MockPolicyHandler)
        .await
        .expect("create agent");

    let reply = agent.chat_text("Run sensitive action").await.expect("chat");
    assert_eq!(reply, "Action authorized and completed.");
    assert!(pre_turn_called.load(Ordering::SeqCst));

    agent.shutdown().await.expect("shutdown");
    server_task.await.expect("server task completed");

    // NOLINT: cleanup in test may fail if mock binary was already cleaned up
    let _ = fs::remove_file(&mock_bin_path);
}

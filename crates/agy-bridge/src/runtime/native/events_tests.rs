use super::*;
use crate::{
    content::{Audio, Document, Image, Video},
    hooks::{HookResult, Hooks},
};

#[test]
fn test_to_domain_step_states_and_sources() {
    let active_user = proto::localharness::StepUpdate {
        step_index: 1,
        state: 1,
        source: 2,
        text: "Hello".to_string(),
        ..Default::default()
    };
    let step = to_domain_step(&active_user);
    assert_eq!(step.status, StepStatus::Active);
    assert_eq!(step.source, StepSource::User);
    assert_eq!(step.step_type, StepType::TextResponse);
    assert_eq!(step.content, "Hello");

    let waiting_system = proto::localharness::StepUpdate {
        step_index: 2,
        state: 3,
        source: 1,
        thinking: "Thinking...".to_string(),
        ..Default::default()
    };
    let step = to_domain_step(&waiting_system);
    assert_eq!(step.status, StepStatus::WaitingForUser);
    assert_eq!(step.source, StepSource::System);
    assert_eq!(step.step_type, StepType::Thinking);
    assert_eq!(step.thinking, "Thinking...");

    let error_model = proto::localharness::StepUpdate {
        step_index: 3,
        state: 4,
        source: 0,
        error_message: "Boom".to_string(),
        ..Default::default()
    };
    let step = to_domain_step(&error_model);
    assert_eq!(step.status, StepStatus::Error);
    assert_eq!(step.source, StepSource::Model);
    assert_eq!(step.error, "Boom");
}

#[test]
fn test_to_domain_step_finish_and_compaction() {
    let compaction = proto::localharness::StepUpdate {
        step_index: 1,
        compaction: Some(proto::localharness::ActionCompaction {}),
        ..Default::default()
    };
    let step = to_domain_step(&compaction);
    assert_eq!(step.step_type, StepType::Compaction);

    let finish_valid = proto::localharness::StepUpdate {
        step_index: 2,
        finish: Some(proto::localharness::ActionFinish {
            output_string: "{\"key\": \"value\"}".to_string(),
        }),
        ..Default::default()
    };
    let step = to_domain_step(&finish_valid);
    assert_eq!(step.step_type, StepType::Finish);
    assert_eq!(
        step.structured_output,
        Some(serde_json::json!({"key": "value"}))
    );

    let finish_invalid = proto::localharness::StepUpdate {
        step_index: 3,
        finish: Some(proto::localharness::ActionFinish {
            output_string: "not json".to_string(),
        }),
        ..Default::default()
    };
    let step = to_domain_step(&finish_invalid);
    assert_eq!(step.step_type, StepType::Finish);
    assert!(step.structured_output.is_none());
}

#[test]
fn test_to_proto_user_input_all_media_types() {
    let img = Image::png(vec![1, 2, 3]);
    let aud = Audio::mp3(vec![4, 5, 6]);
    let doc = Document::pdf(vec![7, 8, 9]);
    let vid = Video::mp4(vec![10, 11, 12]);

    let content = Content::Multi {
        parts: vec![
            ContentPrimitive::Text {
                text: "Prompt".to_string(),
            },
            ContentPrimitive::Image(img),
            ContentPrimitive::Audio(aud),
            ContentPrimitive::Document(doc),
            ContentPrimitive::Video(vid),
        ],
    };

    let user_input = to_proto_user_input(&content);
    assert_eq!(user_input.parts.len(), 5);
}

#[test]
fn test_dispatch_pre_turn_and_pre_tool_hooks() {
    let mut hooks = Hooks::new();
    hooks.on_pre_turn("log_turn", |_ctx| HookResult::allow());
    hooks.on_pre_tool_call_decide("deny_all", |_ctx| HookResult::deny("blocked"));

    let turn_req = proto::localharness::CallHookRequest {
        request_id: "req-1".to_string(),
        name: "pre_turn".to_string(),
        r#type: 3,
        client_id: String::new(),
        args: Some(proto::localharness::call_hook_request::Args::PreTurnArgs(
            proto::localharness::PreTurnArgs {
                user_input: Some(proto::localharness::UserInput {
                    parts: vec![proto::localharness::user_input::Part {
                        part: Some(proto::localharness::user_input::part::Part::Text(
                            "Hello".to_string(),
                        )),
                    }],
                }),
                trajectory_id: String::new(),
            },
        )),
    };
    let turn_outcome = dispatch_pre_turn_hook(&hooks, &turn_req, 1);
    match turn_outcome {
        proto::localharness::call_hook_response::Result::PreTurnResult(res) => {
            assert_eq!(res.decision, 1); // ALLOW
        }
        other => panic!("Expected PreTurnResult, got: {other:?}"),
    }

    let tool_req = proto::localharness::CallHookRequest {
        request_id: "req-2".to_string(),
        name: "pre_tool".to_string(),
        r#type: 5,
        client_id: String::new(),
        args: Some(proto::localharness::call_hook_request::Args::PreToolArgs(
            proto::localharness::PreToolArgs {
                tool_name: "write_file".to_string(),
                arguments_json: "{}".to_string(),
                server_name: String::new(),
                call_id: "call-1".to_string(),
                trajectory_id: String::new(),
                step_index: 0,
            },
        )),
    };
    let tool_outcome = dispatch_pre_tool_hook(&hooks, &tool_req);
    match tool_outcome {
        proto::localharness::call_hook_response::Result::PreToolResult(res) => {
            assert_eq!(res.decision, 2); // DENY
            assert_eq!(res.reason, "blocked");
        }
        other => panic!("Expected PreToolResult, got: {other:?}"),
    }
}

#[tokio::test]
async fn test_handle_trajectory_state_update_flow() {
    let turn_count = Arc::new(AtomicU32::new(0));
    let is_idle = Arc::new(AtomicBool::new(false));
    let idle_notify = Arc::new(Notify::new());
    let active_writer = Arc::new(tokio::sync::Mutex::new(None));
    let last_error = Arc::new(Mutex::new(None));
    let produced_output = Arc::new(AtomicBool::new(true));
    let turn_activity = Arc::new(AtomicBool::new(true));
    let history = Arc::new(Mutex::new(Vec::new()));
    let last_response_text = Arc::new(Mutex::new(Some("Final reply".to_string())));

    let ctx = TrajectoryStateContext {
        turn_count: &turn_count,
        is_idle: &is_idle,
        idle_notify: &idle_notify,
        active_writer: &active_writer,
        last_error: &last_error,
        produced_output: &produced_output,
        turn_activity: &turn_activity,
        history: &history,
        last_response_text: &last_response_text,
    };

    let state_update = proto::localharness::TrajectoryStateUpdate {
        state: 2, // Terminal/Idle state
        error: String::new(),
        trajectory_id: "traj-1".to_string(),
        ..Default::default()
    };

    handle_trajectory_state_update(state_update, &ctx).await;

    assert_eq!(turn_count.load(Ordering::SeqCst), 1);
    assert!(is_idle.load(Ordering::SeqCst));
    let hist = history.lock().unwrap();
    assert_eq!(hist.len(), 1);
    assert_eq!(hist[0].content, "Final reply");
    assert_eq!(hist[0].role, MessageRole::Model);
}

#[tokio::test]
async fn test_empty_completion_without_activity_yields_error() {
    use crate::streaming::channel_with_buffer;

    // A turn that completes FULLY_IDLE with no output, no captured error,
    // and no harness activity must surface an error to the consumer rather
    // than silently closing the stream (which would read as `Ok("")`).
    let (writer, handle) = channel_with_buffer(16);
    let turn_count = Arc::new(AtomicU32::new(0));
    let is_idle = Arc::new(AtomicBool::new(false));
    let idle_notify = Arc::new(Notify::new());
    let active_writer = Arc::new(tokio::sync::Mutex::new(Some(writer)));
    let last_error = Arc::new(Mutex::new(None));
    let produced_output = Arc::new(AtomicBool::new(false));
    let turn_activity = Arc::new(AtomicBool::new(false));
    let history = Arc::new(Mutex::new(Vec::new()));
    let last_response_text = Arc::new(Mutex::new(None));

    let ctx = TrajectoryStateContext {
        turn_count: &turn_count,
        is_idle: &is_idle,
        idle_notify: &idle_notify,
        active_writer: &active_writer,
        last_error: &last_error,
        produced_output: &produced_output,
        turn_activity: &turn_activity,
        history: &history,
        last_response_text: &last_response_text,
    };

    let state_update = proto::localharness::TrajectoryStateUpdate {
        state: TRAJECTORY_STATE_FULLY_IDLE,
        error: String::new(),
        trajectory_id: "traj-empty".to_string(),
        ..Default::default()
    };

    handle_trajectory_state_update(state_update, &ctx).await;

    handle
        .text()
        .await
        .expect_err("Empty completion with no activity must be an error");
}

#[tokio::test]
async fn test_empty_completion_with_activity_is_ok() {
    use crate::streaming::channel_with_buffer;

    // If the harness produced activity during the turn (e.g. a tool-only or
    // structured-output turn) an empty text completion is legitimate and
    // must NOT be turned into an error.
    let (writer, handle) = channel_with_buffer(16);
    let turn_count = Arc::new(AtomicU32::new(0));
    let is_idle = Arc::new(AtomicBool::new(false));
    let idle_notify = Arc::new(Notify::new());
    let active_writer = Arc::new(tokio::sync::Mutex::new(Some(writer)));
    let last_error = Arc::new(Mutex::new(None));
    let produced_output = Arc::new(AtomicBool::new(false));
    let turn_activity = Arc::new(AtomicBool::new(true));
    let history = Arc::new(Mutex::new(Vec::new()));
    let last_response_text = Arc::new(Mutex::new(None));

    let ctx = TrajectoryStateContext {
        turn_count: &turn_count,
        is_idle: &is_idle,
        idle_notify: &idle_notify,
        active_writer: &active_writer,
        last_error: &last_error,
        produced_output: &produced_output,
        turn_activity: &turn_activity,
        history: &history,
        last_response_text: &last_response_text,
    };

    let state_update = proto::localharness::TrajectoryStateUpdate {
        state: TRAJECTORY_STATE_FULLY_IDLE,
        error: String::new(),
        trajectory_id: "traj-active".to_string(),
        ..Default::default()
    };

    handle_trajectory_state_update(state_update, &ctx).await;

    let result = handle
        .text()
        .await
        .expect("Empty completion with harness activity must be Ok");
    assert_eq!(result.into_string(), "");
}

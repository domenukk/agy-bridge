//! Protobuf event conversions and message handlers for Direct runtime.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU32, Ordering},
};

use tokio::sync::{Notify, mpsc};

use crate::{
    agent::AgentId,
    content::{Content, ContentPrimitive},
    hooks::{
        Hooks, OnToolErrorContext, PostToolCallContext, PostTurnContext, PreToolCallDecideContext,
        PreTurnContext,
    },
    proto,
    runtime::bridge_state::bridge_state,
    streaming::{ChatResponseWriter, ResponseEvent, StreamChunk, ToolCallEvent},
    tools::{ToolContext, ToolRegistry},
    types::{
        ConversationMessage, MessageRole, Step, StepSource, StepStatus, StepType, UsageMetadata,
    },
};

/// `TrajectoryStateUpdate.State::STATE_FULLY_IDLE` protobuf wire value.
const TRAJECTORY_STATE_FULLY_IDLE: i32 = 2;

fn to_domain_modality(m: i32) -> crate::types::Modality {
    match m {
        1 => crate::types::Modality::Text,
        2 => crate::types::Modality::Image,
        3 => crate::types::Modality::Video,
        4 => crate::types::Modality::Audio,
        5 => crate::types::Modality::Document,
        _ => crate::types::Modality::Unspecified,
    }
}

fn to_domain_modality_counts(
    counts: &[proto::localharness::ModalityTokenCount],
) -> Vec<crate::types::ModalityTokenCount> {
    counts
        .iter()
        .map(|c| crate::types::ModalityTokenCount {
            modality: to_domain_modality(c.modality),
            token_count: c.token_count,
        })
        .collect()
}

pub(crate) fn to_usage_metadata(u: &proto::localharness::UsageMetadata) -> UsageMetadata {
    UsageMetadata {
        prompt_token_count: Some(u.prompt_token_count),
        candidates_token_count: Some(u.candidates_token_count),
        total_token_count: Some(u.total_token_count),
        cached_content_token_count: Some(u.cached_content_token_count),
        thoughts_token_count: Some(u.thoughts_token_count),
        prompt_tokens_details: to_domain_modality_counts(&u.prompt_tokens_details),
        cache_tokens_details: to_domain_modality_counts(&u.cache_tokens_details),
        candidates_tokens_details: to_domain_modality_counts(&u.candidates_tokens_details),
        tool_use_prompt_tokens_details: to_domain_modality_counts(
            &u.tool_use_prompt_tokens_details,
        ),
        service_tier: if u.service_tier.is_empty() {
            None
        } else {
            match u.service_tier.parse::<crate::types::ServiceTier>() {
                Ok(tier) => Some(tier),
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        service_tier = %u.service_tier,
                        "unrecognized service_tier in usage metadata"
                    );
                    None
                }
            }
        },
    }
}

pub(crate) fn to_domain_step(update: &proto::localharness::StepUpdate) -> Step {
    let status = match update.state {
        1 => StepStatus::Active,
        3 => StepStatus::WaitingForUser,
        4 => StepStatus::Error,
        _ => StepStatus::Done,
    };

    let source = match update.source {
        2 => StepSource::User,
        1 => StepSource::System,
        _ => StepSource::Model,
    };

    let (step_type, structured_output) = if update.compaction.is_some() {
        (StepType::Compaction, None)
    } else if let Some(ref finish) = update.finish {
        let structured = if finish.output_string.is_empty() {
            None
        } else {
            match serde_json::from_str::<serde_json::Value>(&finish.output_string) {
                Ok(val) => Some(val),
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to parse finish output_string as JSON");
                    None
                }
            }
        };
        (StepType::Finish, structured)
    } else if !update.text.is_empty() {
        (StepType::TextResponse, None)
    } else if !update.thinking.is_empty() {
        (StepType::Thinking, None)
    } else {
        (StepType::Unknown, None)
    };

    let mut step = Step::builder()
        .id(format!("step-{}", update.step_index))
        .step_index(update.step_index)
        .cascade_id(update.cascade_id.clone())
        .trajectory_id(update.trajectory_id.clone())
        .parent_trajectory_id(update.parent_trajectory_id.clone())
        .step_type(step_type)
        .status(status)
        .source(source)
        .content(update.text.clone())
        .thinking(update.thinking.clone())
        .error(update.error_message.clone())
        .build();
    step.structured_output = structured_output;
    step
}

fn to_proto_media_part(mime_type: String, data: Vec<u8>) -> proto::localharness::user_input::Part {
    proto::localharness::user_input::Part {
        part: Some(proto::localharness::user_input::part::Part::Media(
            proto::localharness::user_input::Media {
                mime_type,
                description: String::new(),
                data,
            },
        )),
    }
}

fn to_proto_text_part(text: String) -> proto::localharness::user_input::Part {
    proto::localharness::user_input::Part {
        part: Some(proto::localharness::user_input::part::Part::Text(text)),
    }
}

fn to_proto_primitive_part(p: &ContentPrimitive) -> proto::localharness::user_input::Part {
    match p {
        ContentPrimitive::Text { text } => to_proto_text_part(text.clone()),
        ContentPrimitive::Image(img) => {
            to_proto_media_part(img.mime_type.clone(), img.data.clone())
        }
        ContentPrimitive::Audio(audio) => {
            to_proto_media_part(audio.mime_type.clone(), audio.data.clone())
        }
        ContentPrimitive::Document(doc) => {
            to_proto_media_part(doc.mime_type.clone(), doc.data.clone())
        }
        ContentPrimitive::Video(video) => {
            to_proto_media_part(video.mime_type.clone(), video.data.clone())
        }
    }
}

pub(crate) fn to_proto_user_input(content: &Content) -> proto::localharness::UserInput {
    let parts = match content {
        Content::Text { text } => vec![to_proto_text_part(text.clone())],
        Content::Image(img) => vec![to_proto_media_part(img.mime_type.clone(), img.data.clone())],
        Content::Audio(audio) => vec![to_proto_media_part(
            audio.mime_type.clone(),
            audio.data.clone(),
        )],
        Content::Document(doc) => {
            vec![to_proto_media_part(doc.mime_type.clone(), doc.data.clone())]
        }
        Content::Video(video) => vec![to_proto_media_part(
            video.mime_type.clone(),
            video.data.clone(),
        )],
        Content::Multi { parts: primitives } => {
            primitives.iter().map(to_proto_primitive_part).collect()
        }
    };

    proto::localharness::UserInput { parts }
}

async fn dispatch_text_delta(
    delta: &str,
    active_writer_clone: &Arc<tokio::sync::Mutex<Option<ChatResponseWriter>>>,
) {
    let guard = active_writer_clone.lock().await;
    if let Some(ref writer) = *guard {
        ChatResponseWriter::fan_out(
            &writer.subs.text,
            &writer.text_tx,
            delta.to_string(),
            "text",
        )
        .await;
        ChatResponseWriter::fan_out(
            &writer.subs.chunk,
            &writer.chunk_tx,
            StreamChunk::Text(delta.to_string()),
            "chunk",
        )
        .await;
        ChatResponseWriter::fan_out(
            &writer.subs.event,
            &writer.event_tx,
            ResponseEvent::TextChunk(delta.to_string()),
            "event",
        )
        .await;
    }
}

async fn dispatch_thinking_delta(
    delta: &str,
    active_writer_clone: &Arc<tokio::sync::Mutex<Option<ChatResponseWriter>>>,
) {
    let guard = active_writer_clone.lock().await;
    if let Some(ref writer) = *guard {
        ChatResponseWriter::fan_out(
            &writer.subs.thought,
            &writer.thought_tx,
            delta.to_string(),
            "thought",
        )
        .await;
        ChatResponseWriter::fan_out(
            &writer.subs.chunk,
            &writer.chunk_tx,
            StreamChunk::Thought(delta.to_string()),
            "chunk",
        )
        .await;
        ChatResponseWriter::fan_out(
            &writer.subs.event,
            &writer.event_tx,
            ResponseEvent::ThoughtChunk(delta.to_string()),
            "event",
        )
        .await;
    }
}

fn record_step_error(
    step_update: &proto::localharness::StepUpdate,
    last_error_clone: &Arc<Mutex<Option<crate::streaming::StreamError>>>,
) {
    let (error_msg, http_code) = if let Some(ref err) = step_update.error {
        let code = match u16::try_from(err.http_code) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, raw_code = err.http_code, "Invalid http_code in step error");
                0
            }
        };
        (err.error_message.clone(), code)
    } else if step_update.error_message.is_empty() {
        ("Unknown step error".to_string(), 0)
    } else {
        (step_update.error_message.clone(), 0)
    };
    match last_error_clone.lock() {
        Ok(mut lock) => {
            *lock = Some(crate::streaming::StreamError::with_http_code(
                error_msg, http_code,
            ));
        }
        Err(e) => {
            tracing::error!(error = %e, "Poisoned last_error lock in record_step_error");
        }
    }
}

pub(crate) async fn handle_step_update(
    agent_id: AgentId,
    step_update: proto::localharness::StepUpdate,
    active_writer_clone: &Arc<tokio::sync::Mutex<Option<ChatResponseWriter>>>,
    last_response_text_clone: &Arc<Mutex<Option<String>>>,
    last_error_clone: &Arc<Mutex<Option<crate::streaming::StreamError>>>,
    produced_output_clone: &Arc<AtomicBool>,
    turn_activity_clone: &Arc<AtomicBool>,
) {
    // The harness emitted a step for this turn: record activity so an abnormal
    // empty completion can be distinguished from a silent failed trajectory.
    turn_activity_clone.store(true, Ordering::SeqCst);
    let is_model_source = step_update.source == 3 || step_update.source == 0;
    if is_model_source && !step_update.text_delta.is_empty() {
        produced_output_clone.store(true, Ordering::SeqCst);
        dispatch_text_delta(&step_update.text_delta, active_writer_clone).await;
        match last_response_text_clone.lock() {
            Ok(mut lock) => {
                if let Some(ref mut existing) = *lock {
                    existing.push_str(&step_update.text_delta);
                } else {
                    *lock = Some(step_update.text_delta.clone());
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "Poisoned last_response_text lock in handle_step_update");
            }
        }
    }
    if is_model_source && !step_update.thinking_delta.is_empty() {
        dispatch_thinking_delta(&step_update.thinking_delta, active_writer_clone).await;
    }

    if is_model_source && !step_update.text.is_empty() {
        produced_output_clone.store(true, Ordering::SeqCst);
        match last_response_text_clone.lock() {
            Ok(mut lock) => *lock = Some(step_update.text.clone()),
            Err(e) => {
                tracing::error!(error = %e, "Poisoned last_response_text lock in handle_step_update");
            }
        }
    }

    if let Some(ref finish) = step_update.finish
        && !finish.output_string.is_empty()
    {
        match serde_json::from_str::<serde_json::Value>(&finish.output_string) {
            Ok(val) => {
                let guard = active_writer_clone.lock().await;
                if let Some(ref writer) = *guard {
                    writer.set_structured_output(val);
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to parse structured output from finish event");
            }
        }
    }

    if step_update.state == 4
        || step_update.error.is_some()
        || !step_update.error_message.is_empty()
    {
        record_step_error(&step_update, last_error_clone);
    }

    if !step_update.cascade_id.is_empty()
        && let Err(e) = crate::runtime::bridge_state::set_agent_conversation_id(
            agent_id,
            step_update.cascade_id.clone(),
        )
    {
        tracing::warn!(error = %e, "Failed to update agent conversation id from step_update cascade_id");
    }

    let step = to_domain_step(&step_update);
    let guard = active_writer_clone.lock().await;
    if let Some(ref writer) = *guard {
        ChatResponseWriter::fan_out(&writer.subs.step, &writer.step_tx, step, "step").await;
    }
}

async fn emit_tool_call_stream_events(
    tool_name: &str,
    call_id: &str,
    args_json: &str,
    active_writer_clone: &Arc<tokio::sync::Mutex<Option<ChatResponseWriter>>>,
) {
    let guard = active_writer_clone.lock().await;
    if let Some(ref writer) = *guard {
        let parsed_args: serde_json::Value =
            serde_json::from_str(args_json).unwrap_or(serde_json::Value::Null);
        let tc_event = ToolCallEvent {
            name: tool_name.to_string(),
            args: parsed_args,
            id: Some(call_id.to_string()),
            canonical_path: None,
        };
        ChatResponseWriter::fan_out(
            &writer.subs.tool_call,
            &writer.tool_call_tx,
            tc_event.clone(),
            "tool_call",
        )
        .await;
        ChatResponseWriter::fan_out(
            &writer.subs.chunk,
            &writer.chunk_tx,
            StreamChunk::ToolCall(tc_event.clone()),
            "chunk",
        )
        .await;
        ChatResponseWriter::fan_out(
            &writer.subs.event,
            &writer.event_tx,
            ResponseEvent::ToolCall(tc_event),
            "event",
        )
        .await;
    }
}

async fn run_tool_pre_check(
    agent_id: AgentId,
    tool_name: &str,
    args_json: &str,
    hr_opt: Option<&Arc<Hooks>>,
) -> Result<serde_json::Value, String> {
    let tool_name_owned = tool_name.to_string();
    let args_json_owned = args_json.to_string();
    let hr_clone = hr_opt.cloned();

    let pre_check = tokio::task::spawn_blocking(move || {
        match crate::runtime::bridge_state::check_tool_execution_allowed(
            agent_id,
            &tool_name_owned,
            &args_json_owned,
        ) {
            Ok(true) => {}
            Ok(false) => {
                return Err(format!(
                    "[POLICY_DENIED] Execution of tool '{tool_name_owned}' was blocked by agent policy rules. Do NOT retry this tool call with the same arguments."
                ));
            }
            Err(e) => {
                return Err(e.to_string());
            }
        }

        let mut args_val: serde_json::Value =
            serde_json::from_str(&args_json_owned).unwrap_or(serde_json::Value::Null);

        if let Some(hr) = hr_clone {
            let ctx = PreToolCallDecideContext::new(&tool_name_owned, args_val.clone());
            let res = hr.run_pre_tool_call_decide(&ctx);
            if !res.allow {
                return Err(res.message);
            }
            args_val = hr.run_transform_tool_input(&ctx);
        }

        Ok(args_val)
    })
    .await;

    match pre_check {
        Ok(res) => res,
        Err(e) => {
            tracing::error!(error = %e, "pre_check panicked in spawn_blocking");
            Err(format!("Internal error in tool pre-check: {e}"))
        }
    }
}

async fn execute_custom_tool(
    agent_id: AgentId,
    tool_name: &str,
    args_json: &str,
    reg: &ToolRegistry,
    hr_opt: Option<&Arc<Hooks>>,
    tool_state: llm_tool::SharedState,
    conv_id: Option<&String>,
) -> (String, String) {
    let args_val = match run_tool_pre_check(agent_id, tool_name, args_json, hr_opt).await {
        Ok(val) => val,
        Err(msg) => return (String::new(), msg),
    };

    let mut tool_ctx = ToolContext::new().with_shared_state(tool_state);
    if let Some(id) = conv_id {
        tool_ctx = tool_ctx.with_conversation_id(id);
    }

    match reg.dispatch(tool_name, args_val.clone(), &tool_ctx).await {
        Ok(out) => {
            crate::runtime::bridge_state::clear_last_tool_error(agent_id);
            let content_str = out.content().to_string();
            let metadata = serde_json::to_value(out.metadata()).unwrap_or(serde_json::Value::Null);
            crate::runtime::bridge_state::record_last_tool_output(agent_id, args_val, metadata);
            let response_json = match serde_json::from_str::<serde_json::Value>(&content_str) {
                Ok(val) => {
                    if val.is_object() {
                        content_str
                    } else {
                        serde_json::json!({ "result": val }).to_string()
                    }
                }
                Err(err) => {
                    tracing::trace!(error = %err, "Tool output is plain text, wrapping in result object");
                    serde_json::json!({ "result": content_str }).to_string()
                }
            };
            (response_json, String::new())
        }
        Err(err) => {
            crate::runtime::bridge_state::record_last_tool_error(agent_id, &err);
            let err_str = err.to_string();
            (String::new(), err_str)
        }
    }
}

pub(crate) async fn handle_tool_call(
    agent_id: AgentId,
    tool_call: proto::localharness::ToolCall,
    active_writer_clone: &Arc<tokio::sync::Mutex<Option<ChatResponseWriter>>>,
    event_tx: &mpsc::Sender<proto::localharness::InputEvent>,
    turn_activity_clone: &Arc<AtomicBool>,
) {
    turn_activity_clone.store(true, Ordering::SeqCst);
    let tool_name = tool_call.name.clone();
    let call_id = tool_call.id.clone();
    let args_json = tool_call.arguments_json.clone();

    emit_tool_call_stream_events(&tool_name, &call_id, &args_json, active_writer_clone).await;

    let (reg_opt, hr_opt, tool_state, conv_id) = match bridge_state().read() {
        Ok(state_guard) => {
            let bridge_entry = state_guard.get(&agent_id);
            (
                bridge_entry.and_then(|e| e.registry.clone()),
                bridge_entry.and_then(|e| e.hook_runner.clone()),
                bridge_entry
                    .map(|e| e.tool_state.clone())
                    // NOLINT: default JSON null/empty value for uninitialized tool state
                    .unwrap_or_default(),
                bridge_entry.and_then(|e| match e.conversation_id.lock() {
                    Ok(guard) => guard.clone(),
                    Err(err) => {
                        tracing::error!(error = %err, "Poisoned conversation_id lock in handle_tool_call");
                        None
                    }
                }),
            )
        }
        Err(err) => {
            tracing::error!(error = %err, "Poisoned bridge_state read lock in handle_tool_call");
            (None, None, llm_tool::SharedState::default(), None)
        }
    };

    let (response_json, error_msg) = if let Some(reg) = reg_opt {
        execute_custom_tool(
            agent_id,
            &tool_name,
            &args_json,
            &reg,
            hr_opt.as_ref(),
            tool_state,
            conv_id.as_ref(),
        )
        .await
    } else {
        (String::new(), format!("Unknown custom tool {tool_name}"))
    };

    let resp_event = proto::localharness::InputEvent {
        event: Some(proto::localharness::input_event::Event::ToolResponse(
            proto::localharness::ToolResponse {
                id: call_id,
                response_json,
                error_message: error_msg,
                response: None,
                supplemental_media: Vec::new(),
            },
        )),
    };
    if let Err(e) = event_tx.send(resp_event).await {
        tracing::error!(error = %e, "Failed to send ToolResponse event");
    }
}

fn dispatch_pre_turn_hook(
    hr: &crate::hooks::Hooks,
    req: &proto::localharness::CallHookRequest,
    turn: u32,
) -> proto::localharness::call_hook_response::Result {
    let prompt = match req.args {
        Some(proto::localharness::call_hook_request::Args::PreTurnArgs(ref a)) => {
            if let Some(ref ui) = a.user_input {
                ui.parts
                    .iter()
                    .filter_map(|p| match &p.part {
                        Some(proto::localharness::user_input::part::Part::Text(t)) => {
                            Some(t.as_str())
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            } else {
                String::new()
            }
        }
        _ => String::new(),
    };
    let ctx = PreTurnContext::new(prompt, turn);
    let res = hr.run_pre_turn(&ctx);
    proto::localharness::call_hook_response::Result::PreTurnResult(
        proto::localharness::PreTurnResult {
            decision: if res.allow { 1 } else { 2 },
            reason: res.message,
        },
    )
}

fn dispatch_post_turn_hook(
    hr: &crate::hooks::Hooks,
    req: &proto::localharness::CallHookRequest,
    turn: u32,
) -> proto::localharness::call_hook_response::Result {
    let response_text = match req.args {
        Some(proto::localharness::call_hook_request::Args::PostTurnArgs(ref a)) => {
            a.response_text.clone()
        }
        _ => String::new(),
    };
    let ctx = PostTurnContext {
        response_text,
        turn_number: turn,
    };
    hr.run_post_turn(&ctx);
    proto::localharness::call_hook_response::Result::EmptyResult(
        proto::localharness::EmptyResult {},
    )
}

fn dispatch_pre_tool_hook(
    hr: &crate::hooks::Hooks,
    req: &proto::localharness::CallHookRequest,
) -> proto::localharness::call_hook_response::Result {
    let (tool_name, args_val) = match req.args {
        Some(proto::localharness::call_hook_request::Args::PreToolArgs(ref a)) => {
            let parsed: serde_json::Value =
                serde_json::from_str(&a.arguments_json).unwrap_or(serde_json::Value::Null);
            (a.tool_name.clone(), parsed)
        }
        _ => (String::new(), serde_json::Value::Null),
    };
    let ctx = PreToolCallDecideContext::new(tool_name, args_val);
    let res = hr.run_pre_tool_call_decide(&ctx);
    if !res.allow {
        return proto::localharness::call_hook_response::Result::PreToolResult(
            proto::localharness::PreToolResult {
                decision: 2,
                reason: res.message,
                modified_arguments_json: String::new(),
            },
        );
    }
    let transformed = hr.run_transform_tool_input(&ctx);
    let modified_arguments_json = if transformed == ctx.tool_args {
        String::new()
    } else {
        transformed.to_string()
    };
    proto::localharness::call_hook_response::Result::PreToolResult(
        proto::localharness::PreToolResult {
            decision: 1,
            reason: res.message,
            modified_arguments_json,
        },
    )
}

fn dispatch_post_tool_hook(
    agent_id: AgentId,
    hr: &crate::hooks::Hooks,
    req: &proto::localharness::CallHookRequest,
) -> proto::localharness::call_hook_response::Result {
    let (tool_name, result) = match req.args {
        Some(proto::localharness::call_hook_request::Args::PostToolArgs(ref a)) => {
            (a.tool_name.clone(), a.result.clone())
        }
        _ => (String::new(), String::new()),
    };
    let (tool_args, metadata) = crate::runtime::bridge_state::take_last_tool_output(agent_id)
        .unwrap_or((serde_json::Value::Null, serde_json::Value::Null));
    let ctx = PostToolCallContext {
        tool_name,
        tool_args,
        result,
        metadata,
    };
    hr.run_post_tool_call(&ctx);
    proto::localharness::call_hook_response::Result::EmptyResult(
        proto::localharness::EmptyResult {},
    )
}

fn dispatch_on_tool_error_hook(
    agent_id: AgentId,
    hr: &crate::hooks::Hooks,
    req: &proto::localharness::CallHookRequest,
) -> proto::localharness::call_hook_response::Result {
    let (tool_name, error_message) = match req.args {
        Some(proto::localharness::call_hook_request::Args::OnToolErrorArgs(ref a)) => {
            (a.tool_name.clone(), a.error_message.clone())
        }
        _ => (String::new(), String::new()),
    };
    let metadata = crate::runtime::bridge_state::take_last_tool_error(agent_id)
        .unwrap_or(serde_json::Value::Null);
    let ctx = OnToolErrorContext {
        tool_name,
        tool_args: serde_json::Value::Null,
        error: error_message,
        metadata,
    };
    let res = hr.run_on_tool_error(&ctx);
    // NOLINT: empty string default for proto custom_error_message
    let custom_error_message = res.unwrap_or_default();
    proto::localharness::call_hook_response::Result::OnToolErrorResult(
        proto::localharness::OnToolErrorResult {
            custom_error_message,
        },
    )
}

fn dispatch_on_session_start_hook(
    agent_id: AgentId,
    hr: &crate::hooks::Hooks,
) -> proto::localharness::call_hook_response::Result {
    let session_id = crate::runtime::bridge_state::get_agent_conversation_id(agent_id)
        .unwrap_or_else(|| format!("agent-{agent_id}"));
    let ctx = crate::hooks::OnSessionStartContext {
        session: crate::hooks::SessionContext {
            session_id,
            agent_id,
            started_at: std::time::SystemTime::now(),
        },
    };
    hr.run_on_session_start(&ctx);
    proto::localharness::call_hook_response::Result::EmptyResult(
        proto::localharness::EmptyResult {},
    )
}

fn dispatch_on_session_end_hook(
    agent_id: AgentId,
    hr: &crate::hooks::Hooks,
) -> proto::localharness::call_hook_response::Result {
    let session_id = crate::runtime::bridge_state::get_agent_conversation_id(agent_id)
        .unwrap_or_else(|| format!("agent-{agent_id}"));
    let ctx = crate::hooks::OnSessionEndContext {
        session: crate::hooks::SessionContext {
            session_id,
            agent_id,
            started_at: std::time::SystemTime::now(),
        },
    };
    hr.run_on_session_end(&ctx);
    proto::localharness::call_hook_response::Result::EmptyResult(
        proto::localharness::EmptyResult {},
    )
}

fn parse_stop_args(a: &proto::localharness::StopArgs) -> crate::hooks::StopArgs {
    let stop_reason = match a.stop_reason.parse::<crate::types::StopReason>() {
        Ok(r) => r,
        Err(err) => {
            tracing::warn!(
                error = %err,
                raw_stop_reason = %a.stop_reason,
                "failed to parse stop_reason proto field, defaulting to Unknown"
            );
            crate::types::StopReason::Unknown
        }
    };
    let continuation_count = match u32::try_from(a.continuation_count) {
        Ok(c) => c,
        Err(err) => {
            tracing::warn!(
                error = %err,
                continuation_count = a.continuation_count,
                "failed to convert continuation_count to u32, using 0"
            );
            0
        }
    };
    crate::hooks::StopArgs {
        response_text: a.response_text.clone(),
        trajectory_id: a.trajectory_id.clone(),
        continuation_count,
        stop_reason,
        error_message: a.error_message.clone(),
    }
}

fn dispatch_stop_hook(
    hr: &crate::hooks::Hooks,
    req: &proto::localharness::CallHookRequest,
) -> proto::localharness::call_hook_response::Result {
    let stop_args = match req.args {
        Some(proto::localharness::call_hook_request::Args::StopArgs(ref a)) => parse_stop_args(a),
        _ => crate::hooks::StopArgs::default(),
    };
    let res = hr.run_stop(&stop_args);
    proto::localharness::call_hook_response::Result::StopResult(proto::localharness::StopResult {
        decision: res.decision.to_proto_i32(),
        reason: res.reason,
    })
}

pub(crate) async fn handle_call_hook_request(
    agent_id: AgentId,
    req: proto::localharness::CallHookRequest,
    turn_count_clone: &Arc<AtomicU32>,
    event_tx: &mpsc::Sender<proto::localharness::InputEvent>,
) {
    let hr_opt = match bridge_state().read() {
        Ok(state_guard) => state_guard
            .get(&agent_id)
            .and_then(|e| e.hook_runner.clone()),
        Err(err) => {
            tracing::error!(error = %err, "Poisoned bridge_state read lock in handle_call_hook_request");
            None
        }
    };

    let request_id = req.request_id.clone();
    let hook_result = if let Some(hr) = hr_opt {
        let turn = turn_count_clone.load(Ordering::Relaxed);
        match tokio::task::spawn_blocking(move || match req.r#type {
            1 => Some(dispatch_on_session_start_hook(agent_id, &hr)),
            2 => Some(dispatch_on_session_end_hook(agent_id, &hr)),
            3 => Some(dispatch_pre_turn_hook(&hr, &req, turn)),
            4 => Some(dispatch_post_turn_hook(&hr, &req, turn)),
            5 => Some(dispatch_pre_tool_hook(&hr, &req)),
            6 => Some(dispatch_post_tool_hook(agent_id, &hr, &req)),
            7 => Some(dispatch_on_tool_error_hook(agent_id, &hr, &req)),
            9 => Some(dispatch_stop_hook(&hr, &req)),
            _ => {
                if matches!(
                    req.args,
                    Some(proto::localharness::call_hook_request::Args::StopArgs(_))
                ) {
                    Some(dispatch_stop_hook(&hr, &req))
                } else {
                    Some(
                        proto::localharness::call_hook_response::Result::EmptyResult(
                            proto::localharness::EmptyResult {},
                        ),
                    )
                }
            }
        })
        .await
        {
            Ok(res) => res,
            Err(e) => {
                tracing::error!(error = %e, "Hook execution panicked in spawn_blocking");
                Some(
                    proto::localharness::call_hook_response::Result::EmptyResult(
                        proto::localharness::EmptyResult {},
                    ),
                )
            }
        }
    } else {
        Some(
            proto::localharness::call_hook_response::Result::EmptyResult(
                proto::localharness::EmptyResult {},
            ),
        )
    };

    let hook_resp = proto::localharness::InputEvent {
        event: Some(proto::localharness::input_event::Event::CallHookResponse(
            proto::localharness::CallHookResponse {
                request_id,
                result: hook_result,
            },
        )),
    };
    if let Err(e) = event_tx.send(hook_resp).await {
        tracing::error!(error = %e, "Failed to send CallHookResponse event");
    }
}

pub(crate) async fn handle_policy_decision_request(
    agent_id: AgentId,
    req: proto::localharness::PolicyDecisionRequest,
    event_tx: &mpsc::Sender<proto::localharness::InputEvent>,
) {
    let mut outcome = proto::localharness::PolicyEvaluationOutcome::Allow as i32;
    let mut deny_reason = String::new();
    let ph_opt = match bridge_state().read() {
        Ok(state_guard) => state_guard
            .get(&agent_id)
            .and_then(|e| e.policy_handler.clone()),
        Err(err) => {
            tracing::error!(error = %err, "Poisoned bridge_state read lock in handle_policy_decision_request");
            None
        }
    };

    if let Some(ph) = ph_opt {
        let tool_name = req
            .tool_args
            .as_ref()
            .map_or_else(String::new, |a| a.tool_name.clone());
        let args_str = req
            .tool_args
            .as_ref()
            .map_or("{}", |a| a.arguments_json.as_str());
        let args_val: serde_json::Value =
            serde_json::from_str(args_str).unwrap_or(serde_json::Value::Null);
        let allowed = tokio::task::spawn_blocking(move || ph.confirm(&tool_name, &args_val))
            .await
            .unwrap_or_else(|e| {
                tracing::error!(error = %e, "Policy confirmation panicked in spawn_blocking");
                false
            });
        if !allowed {
            outcome = proto::localharness::PolicyEvaluationOutcome::Deny as i32;
            deny_reason = "Denied by user policy".to_string();
        }
    }

    let decision_resp = proto::localharness::InputEvent {
        event: Some(
            proto::localharness::input_event::Event::PolicyDecisionResponse(
                proto::localharness::PolicyDecisionResponse {
                    request_id: req.request_id,
                    outcome,
                    deny_reason,
                },
            ),
        ),
    };
    if let Err(e) = event_tx.send(decision_resp).await {
        tracing::error!(error = %e, "Failed to send PolicyDecisionResponse event");
    }
}

pub(crate) async fn handle_usage_update(
    usage: proto::localharness::UsageUpdate,
    total_usage_clone: &Arc<Mutex<UsageMetadata>>,
    last_turn_usage_clone: &Arc<Mutex<UsageMetadata>>,
    active_writer_clone: &Arc<tokio::sync::Mutex<Option<ChatResponseWriter>>>,
) {
    if let Some(ref total) = usage.total {
        let u = to_usage_metadata(total);
        match total_usage_clone.lock() {
            Ok(mut lock) => *lock = u.clone(),
            Err(e) => {
                tracing::error!(error = %e, "Poisoned total_usage lock in handle_usage_update");
            }
        }
        match last_turn_usage_clone.lock() {
            Ok(mut lock) => *lock = u.clone(),
            Err(e) => {
                tracing::error!(error = %e, "Poisoned last_turn_usage lock in handle_usage_update");
            }
        }
        let guard = active_writer_clone.lock().await;
        if let Some(ref writer) = *guard {
            writer.set_usage(u);
        }
    }
}

/// Context handles required to process a trajectory state update.
pub(crate) struct TrajectoryStateContext<'a> {
    pub(crate) turn_count: &'a Arc<AtomicU32>,
    pub(crate) is_idle: &'a Arc<AtomicBool>,
    pub(crate) idle_notify: &'a Arc<Notify>,
    pub(crate) active_writer: &'a Arc<tokio::sync::Mutex<Option<ChatResponseWriter>>>,
    pub(crate) last_error: &'a Arc<Mutex<Option<crate::streaming::StreamError>>>,
    pub(crate) produced_output: &'a Arc<AtomicBool>,
    pub(crate) turn_activity: &'a Arc<AtomicBool>,
    pub(crate) history: &'a Arc<Mutex<Vec<ConversationMessage>>>,
    pub(crate) last_response_text: &'a Arc<Mutex<Option<String>>>,
}

pub(crate) async fn handle_trajectory_state_update(
    state_update: proto::localharness::TrajectoryStateUpdate,
    ctx: &TrajectoryStateContext<'_>,
) {
    if matches!(state_update.state, 2..=4) {
        ctx.turn_count.fetch_add(1, Ordering::SeqCst);
        ctx.is_idle.store(true, Ordering::SeqCst);
        ctx.idle_notify.notify_waiters();

        match ctx.last_response_text.lock() {
            Ok(mut lock) => {
                if let Some(text) = lock.take()
                    && !text.is_empty()
                {
                    match ctx.history.lock() {
                        Ok(mut hist) => {
                            hist.push(ConversationMessage {
                                role: MessageRole::Model,
                                content: text,
                            });
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "Poisoned history lock in handle_trajectory_state_update");
                        }
                    }
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "Poisoned last_response_text lock in handle_trajectory_state_update");
            }
        }

        let err_opt = if ctx.produced_output.load(Ordering::SeqCst) {
            None
        } else {
            match ctx.last_error.lock() {
                Ok(mut l) => l.take(),
                Err(e) => {
                    tracing::error!(error = %e, "Poisoned last_error lock in handle_trajectory_state_update");
                    None
                }
            }
        };

        // A turn that returns FULLY_IDLE without producing any output, any
        // captured error, or any harness activity indicates a silently failed
        // trajectory (e.g. a prior backend error left the executor terminal).
        // Surface it as an error instead of a misleading empty `Ok("")`.
        let empty_completion_is_failure = state_update.state == TRAJECTORY_STATE_FULLY_IDLE
            && !ctx.produced_output.load(Ordering::SeqCst)
            && !ctx.turn_activity.load(Ordering::SeqCst);

        let mut guard = ctx.active_writer.lock().await;
        if let Some(writer) = guard.take() {
            if !state_update.error.is_empty() {
                if let Err(e) = writer
                    .send_error(crate::streaming::StreamError::new(&state_update.error))
                    .await
                {
                    tracing::debug!(error = %e, "Failed to send trajectory error to writer");
                }
            } else if let Some(err) = err_opt {
                if let Err(e) = writer.send_error(err).await {
                    tracing::debug!(error = %e, "Failed to send error to writer");
                }
            } else if empty_completion_is_failure {
                let err = crate::streaming::StreamError::new(
                    "Harness turn completed without any output, tool activity, or error; \
                     the trajectory is likely in a failed state and cannot continue",
                );
                if let Err(e) = writer.send_error(err).await {
                    tracing::debug!(error = %e, "Failed to send synthesized empty-completion error");
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "events_tests.rs"]
mod tests;

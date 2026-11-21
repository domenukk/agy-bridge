//! Protobuf event conversions and message handlers for Direct runtime.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
};

use tokio::sync::{Notify, mpsc};

use crate::{
    agent::AgentId,
    hooks::{Hooks, PreToolCallDecideContext},
    proto,
    runtime::bridge_state::{
        bridge_state, clear_tool_error_for_call, invalidate_confirmed_tool_call,
        record_confirmed_tool_call, record_tool_error_for_call, record_tool_output_for_call,
        take_pre_tool_evaluated,
    },
    streaming::{ChatResponseWriter, ResponseEvent, ToolCallEvent},
    tools::{ToolContext, ToolRegistry},
    types::{
        ConversationMessage, MessageRole, Step, StepSource, StepStatus, StepTarget, StepType,
        StopReason, UsageMetadata,
    },
};

mod hooks;
mod interaction;

#[cfg(test)]
use self::hooks::{dispatch_on_compaction_hook, dispatch_pre_tool_hook, dispatch_pre_turn_hook};
use self::interaction::handle_questions_request;
pub(crate) use self::{hooks::handle_call_hook_request, interaction::to_proto_user_input};

/// `TrajectoryStateUpdate.State::STATE_FULLY_IDLE` protobuf wire value.
const TRAJECTORY_STATE_FULLY_IDLE: i32 = 2;
/// `TrajectoryStateUpdate.State::STATE_CANCELLED` protobuf wire value.
const TRAJECTORY_STATE_CANCELLED: i32 = 3;

/// Determine whether an incoming trajectory event belongs to the root conversation
/// (seeding `root_trajectory_id` on the first root event when unset).
pub(crate) fn resolve_is_root_trajectory(
    root_trajectory_id: &Mutex<Option<String>>,
    trajectory_id: &str,
    parent_trajectory_id: &str,
    depth: i32,
) -> bool {
    if !parent_trajectory_id.is_empty() || depth > 0 {
        return false;
    }
    let mut guard = match root_trajectory_id.lock() {
        Ok(g) => g,
        Err(poisoned) => {
            tracing::warn!(error = %poisoned, "root_trajectory_id mutex poisoned; recovering");
            poisoned.into_inner()
        }
    };
    if let Some(ref root_id) = *guard {
        return trajectory_id.is_empty() || trajectory_id == root_id;
    }
    if !trajectory_id.is_empty() {
        *guard = Some(trajectory_id.to_owned());
    }
    true
}

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

    let target = match update.target {
        1 => StepTarget::User,
        2 => StepTarget::Model,
        3 => StepTarget::Environment,
        0 => StepTarget::Unspecified,
        _ => StepTarget::Unknown,
    };

    let tool_calls = super::tools::extract_step_tool_calls(update);

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
    } else if !tool_calls.is_empty() {
        (StepType::ToolCall, None)
    } else if !update.text.is_empty() || !update.text_delta.is_empty() {
        (StepType::TextResponse, None)
    } else if !update.thinking.is_empty() || !update.thinking_delta.is_empty() {
        (StepType::Thinking, None)
    } else {
        (StepType::Unknown, None)
    };

    let depth = u32::from(!update.parent_trajectory_id.is_empty());
    let (error_text, http_code) = match update.error.as_ref() {
        Some(err) => {
            let code = u16::try_from(err.http_code).unwrap_or_else(|e| {
                tracing::debug!(error = %e, raw = err.http_code, "Invalid step error http_code");
                0
            });
            let msg = if err.error_message.is_empty() {
                update.error_message.clone()
            } else {
                err.error_message.clone()
            };
            (msg, code)
        }
        None => (update.error_message.clone(), 0),
    };
    let is_complete_response = source == StepSource::Model
        && status == StepStatus::Done
        && !update.text.is_empty()
        && matches!(target, StepTarget::User | StepTarget::Unspecified);

    let mut step = Step::builder()
        .id(format!("step-{}", update.step_index))
        .step_index(update.step_index)
        .cascade_id(update.cascade_id.clone())
        .trajectory_id(update.trajectory_id.clone())
        .parent_trajectory_id(update.parent_trajectory_id.clone())
        .depth(depth)
        .step_type(step_type)
        .status(status)
        .source(source)
        .target(target)
        .content(update.text.clone())
        .content_delta(update.text_delta.clone())
        .thinking(update.thinking.clone())
        .thinking_delta(update.thinking_delta.clone())
        .tool_calls(tool_calls)
        .error(error_text)
        .http_code(http_code)
        .is_complete_response(is_complete_response)
        .build();
    step.structured_output = structured_output;
    step.workflow_progress =
        update
            .run_workflow
            .as_ref()
            .map(|rw| crate::types::WorkflowProgress {
                script_path: super::tools::normalize_wire_path(&rw.script_path),
                script: rw.script.clone(),
                description: rw.description.clone(),
                output: rw.output.clone(),
            });
    step
}

async fn dispatch_text_delta(
    delta: &str,
    active_writer_clone: &Arc<tokio::sync::Mutex<Option<ChatResponseWriter>>>,
) {
    let guard = active_writer_clone.lock().await;
    if let Some(ref writer) = *guard {
        writer.emit_text(delta).await;
    }
}

async fn dispatch_thinking_delta(
    delta: &str,
    active_writer_clone: &Arc<tokio::sync::Mutex<Option<ChatResponseWriter>>>,
) {
    let guard = active_writer_clone.lock().await;
    if let Some(ref writer) = *guard {
        writer.emit_thought(delta).await;
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

/// Context handles required to process a harness step update.
pub(crate) type BuiltinToolSeenSet = Arc<Mutex<std::collections::HashSet<(u32, String)>>>;
pub(crate) type QuestionStepSeenSet = Arc<Mutex<std::collections::HashSet<(String, u32)>>>;

pub(crate) struct StepUpdateContext<'a> {
    pub(crate) active_writer: &'a Arc<tokio::sync::Mutex<Option<ChatResponseWriter>>>,
    pub(crate) last_response_text: &'a Arc<Mutex<Option<String>>>,
    pub(crate) last_error: &'a Arc<Mutex<Option<crate::streaming::StreamError>>>,
    pub(crate) produced_output: &'a Arc<AtomicBool>,
    pub(crate) turn_activity: &'a Arc<AtomicBool>,
    pub(crate) compaction_indices: &'a Arc<Mutex<Vec<u32>>>,
    pub(crate) root_trajectory_id: &'a Arc<Mutex<Option<String>>>,
    pub(crate) event_tx: Option<&'a mpsc::Sender<proto::localharness::InputEvent>>,
    pub(crate) emitted_builtin_tool_steps: Option<&'a BuiltinToolSeenSet>,
    pub(crate) answered_question_steps: Option<&'a QuestionStepSeenSet>,
}

async fn emit_builtin_step_tool_calls(
    step_update: &proto::localharness::StepUpdate,
    ctx: &StepUpdateContext<'_>,
) {
    if step_update.custom_tool.is_some() {
        return;
    }
    let step_calls = super::tools::extract_step_tool_calls(step_update);
    if step_calls.is_empty() {
        return;
    }
    let guard = ctx.active_writer.lock().await;
    let Some(ref writer) = *guard else {
        return;
    };
    for tc in step_calls {
        let should_emit = match ctx.emitted_builtin_tool_steps {
            Some(seen_mutex) => match seen_mutex.lock() {
                Ok(mut seen) => seen.insert((step_update.step_index, tc.name.clone())),
                Err(poisoned) => poisoned
                    .into_inner()
                    .insert((step_update.step_index, tc.name.clone())),
            },
            None => true,
        };
        if should_emit {
            let id = tc.id.or_else(|| {
                Some(format!(
                    "{}:{}",
                    step_update.trajectory_id, step_update.step_index
                ))
            });
            writer
                .emit_tool_call(ToolCallEvent {
                    name: tc.name,
                    args: tc.args,
                    id,
                    canonical_path: tc.canonical_path,
                })
                .await;
        }
    }
}

async fn apply_root_step_update(
    agent_id: AgentId,
    step_update: &proto::localharness::StepUpdate,
    ctx: &StepUpdateContext<'_>,
) {
    if step_update.compaction.is_some() && step_update.state == 2 {
        match ctx.compaction_indices.lock() {
            Ok(mut indices) => {
                if !indices.contains(&step_update.step_index) {
                    indices.push(step_update.step_index);
                }
            }
            Err(e) => {
                tracing::error!(
                    error = %e,
                    "Poisoned compaction_indices lock in handle_step_update"
                );
            }
        }
    }
    emit_builtin_step_tool_calls(step_update, ctx).await;
    let is_model_user_step = (step_update.source == 3 || step_update.source == 0)
        && (step_update.target == 1 || step_update.target == 0)
        && step_update.state != 4;
    if is_model_user_step && !step_update.text_delta.is_empty() {
        ctx.produced_output.store(true, Ordering::SeqCst);
        dispatch_text_delta(&step_update.text_delta, ctx.active_writer).await;
        match ctx.last_response_text.lock() {
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
    if is_model_user_step && !step_update.thinking_delta.is_empty() {
        dispatch_thinking_delta(&step_update.thinking_delta, ctx.active_writer).await;
    }
    if is_model_user_step && !step_update.text.is_empty() {
        ctx.produced_output.store(true, Ordering::SeqCst);
        match ctx.last_response_text.lock() {
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
                let guard = ctx.active_writer.lock().await;
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
        record_step_error(step_update, ctx.last_error);
    }
    if !step_update.cascade_id.is_empty()
        && let Err(e) = crate::runtime::bridge_state::set_agent_conversation_id(
            agent_id,
            step_update.cascade_id.clone(),
        )
    {
        tracing::warn!(error = %e, "Failed to update agent conversation id from step_update cascade_id");
    }
}

pub(crate) async fn handle_step_update(
    agent_id: AgentId,
    step_update: proto::localharness::StepUpdate,
    ctx: &StepUpdateContext<'_>,
) {
    ctx.turn_activity.store(true, Ordering::SeqCst);
    let is_root = resolve_is_root_trajectory(
        ctx.root_trajectory_id,
        &step_update.trajectory_id,
        &step_update.parent_trajectory_id,
        0,
    );

    if let Some(ref qr) = step_update.questions_request
        && let Some(event_tx) = ctx.event_tx
    {
        let should_answer = match ctx.answered_question_steps {
            Some(seen_mutex) => match seen_mutex.lock() {
                Ok(mut seen) => {
                    seen.insert((step_update.trajectory_id.clone(), step_update.step_index))
                }
                Err(poisoned) => poisoned
                    .into_inner()
                    .insert((step_update.trajectory_id.clone(), step_update.step_index)),
            },
            None => true,
        };
        if should_answer {
            handle_questions_request(agent_id, &step_update, qr, event_tx).await;
        }
    }

    if is_root {
        apply_root_step_update(agent_id, &step_update, ctx).await;
    }

    let guard = ctx.active_writer.lock().await;
    if let Some(ref writer) = *guard
        && writer.wants_step()
    {
        writer.emit_step(to_domain_step(&step_update)).await;
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
        let normalized = super::tools::parse_and_normalize_tool_args(args_json, tool_name);
        let tc_event = ToolCallEvent {
            name: tool_name.to_string(),
            args: normalized.args,
            id: Some(call_id.to_string()),
            canonical_path: normalized.canonical_path,
        };
        writer.emit_tool_call(tc_event).await;
    }
}

/// Wire envelope wrapping non-object custom tool output as `{"result": ...}`.
#[derive(serde::Serialize)]
struct ToolResultEnvelope<T: serde::Serialize> {
    result: T,
}

pub(super) fn parse_json_or_null(json_str: &str, context: &str) -> serde_json::Value {
    if json_str.is_empty() {
        return serde_json::Value::Null;
    }
    match serde_json::from_str(json_str) {
        Ok(val) => val,
        Err(err) => {
            tracing::warn!(error = %err, context, "Failed to parse JSON payload; defaulting to null");
            serde_json::Value::Null
        }
    }
}

/// Recursively neutralize LLM control tokens across all JSON string keys and string values
/// so JSON `\u003c`/`\u003e` escapes cannot decode back into raw `<|im_start|>` delimiters.
pub(crate) fn sanitize_json_strings_recursive(val: &mut serde_json::Value) {
    match val {
        serde_json::Value::String(s) => {
            if !llm_tool::is_quarantined_output(s) {
                let sanitized = llm_tool::neutralize_delimiters(s);
                if sanitized != *s {
                    *s = sanitized;
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                sanitize_json_strings_recursive(item);
            }
        }
        serde_json::Value::Object(map) => {
            let entries: Vec<(String, serde_json::Value)> = std::mem::take(map)
                .into_iter()
                .map(|(k, mut v)| {
                    let clean_key = if llm_tool::is_quarantined_output(&k) {
                        k
                    } else {
                        llm_tool::neutralize_delimiters(&k)
                    };
                    sanitize_json_strings_recursive(&mut v);
                    (clean_key, v)
                })
                .collect();
            for (k, v) in entries {
                map.insert(k, v);
            }
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
    }
}

async fn run_tool_pre_check(
    agent_id: AgentId,
    tool_name: &str,
    call_id: &str,
    args_json: &str,
    hr_opt: Option<&Arc<Hooks>>,
) -> Result<serde_json::Value, String> {
    let tool_name_owned = tool_name.to_string();
    let call_id_owned = call_id.to_string();
    let args_json_owned = args_json.to_string();
    let hr_clone = hr_opt.cloned();

    let pre_check = tokio::task::spawn_blocking(move || {
        let normalized =
            super::tools::try_parse_and_normalize_tool_args(&args_json_owned, "run_tool_pre_check")?;
        let mut args_val = normalized.args;
        let already_pre_tool_evaluated = take_pre_tool_evaluated(agent_id, &call_id_owned);

        if !already_pre_tool_evaluated && let Some(hr) = hr_clone {
            let initial_ctx = PreToolCallDecideContext::new(&tool_name_owned, args_val.clone());
            let mut transformed = hr.run_transform_tool_input(&initial_ctx);
            if transformed != args_val {
                invalidate_confirmed_tool_call(agent_id, &tool_name_owned, &args_val);
                super::tools::normalize_json_object_paths_in_place(&mut transformed)?;
            }
            let decide_ctx = PreToolCallDecideContext::new(&tool_name_owned, transformed.clone());
            let res = hr.run_pre_tool_call_decide(&decide_ctx);
            if !res.allow {
                return Err(res.message);
            }
            args_val = transformed;
        }

        let effective_json = serde_json::to_string(&args_val)
            .map_err(|e| format!("Failed to serialize effective tool args: {e}"))?;
        match crate::runtime::bridge_state::check_tool_execution_allowed(
            agent_id,
            &tool_name_owned,
            &effective_json,
        ) {
            Ok(true) => Ok(args_val),
            Ok(false) => Err(format!(
                "[POLICY_DENIED] Execution of tool '{tool_name_owned}' was blocked by agent policy rules. Do NOT retry this tool call with the same arguments."
            )),
            Err(e) => Err(e.to_string()),
        }
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

struct CustomToolExecInputs<'a> {
    agent_id: AgentId,
    tool_name: &'a str,
    call_id: &'a str,
    args_json: &'a str,
    reg: &'a ToolRegistry,
    hr_opt: Option<&'a Arc<Hooks>>,
    tool_state: llm_tool::SharedState,
    surrogate_vault: llm_tool::SurrogateVault,
    conv_id: Option<&'a String>,
}

async fn execute_custom_tool(inputs: CustomToolExecInputs<'_>) -> (String, String) {
    let floor = inputs.reg.output_floor();
    let args_val = match run_tool_pre_check(
        inputs.agent_id,
        inputs.tool_name,
        inputs.call_id,
        inputs.args_json,
        inputs.hr_opt,
    )
    .await
    {
        Ok(val) => val,
        Err(msg) => return (String::new(), floor.apply_str(&msg).into_owned()),
    };

    let mut tool_ctx = ToolContext::new()
        .with_shared_state(inputs.tool_state)
        .with_surrogate_vault(inputs.surrogate_vault);
    if let Some(id) = inputs.conv_id {
        tool_ctx = tool_ctx.with_conversation_id(id);
    }

    match inputs
        .reg
        .dispatch(inputs.tool_name, args_val.clone(), &tool_ctx)
        .await
    {
        Ok(out) => {
            clear_tool_error_for_call(inputs.agent_id, inputs.call_id);
            let content_str = out.content().to_string();
            let metadata = match serde_json::to_value(out.metadata()) {
                Ok(val) => val,
                Err(err) => {
                    tracing::warn!(error = %err, tool = inputs.tool_name, "Failed to serialize tool metadata; defaulting to null");
                    serde_json::Value::Null
                }
            };
            record_tool_output_for_call(inputs.agent_id, inputs.call_id, args_val, metadata);
            let response_json = match serde_json::from_str::<serde_json::Value>(&content_str) {
                Ok(mut val) => {
                    if floor.sanitizes_tokens() {
                        sanitize_json_strings_recursive(&mut val);
                    }
                    if val.is_object() {
                        serde_json::to_string(&val)
                    } else {
                        serde_json::to_string(&ToolResultEnvelope { result: val })
                    }
                }
                Err(err) => {
                    tracing::trace!(error = %err, "Tool output is plain text, wrapping in result object");
                    serde_json::to_string(&ToolResultEnvelope {
                        result: content_str,
                    })
                }
            };
            match response_json {
                Ok(json) => (json, String::new()),
                Err(err) => {
                    tracing::warn!(error = %err, tool = inputs.tool_name, "Failed to serialize tool result envelope");
                    (
                        String::new(),
                        floor.apply_str(&err.to_string()).into_owned(),
                    )
                }
            }
        }
        Err(err) => {
            record_tool_error_for_call(inputs.agent_id, inputs.call_id, args_val, &err);
            let sanitized_err = floor.apply_str(&err.to_string()).into_owned();
            (String::new(), sanitized_err)
        }
    }
}

/// Shared handles needed to execute a custom tool call and report its result.
pub(crate) struct ToolCallDispatchContext {
    pub(crate) active_writer: Arc<tokio::sync::Mutex<Option<ChatResponseWriter>>>,
    pub(crate) event_tx: mpsc::Sender<proto::localharness::InputEvent>,
    pub(crate) turn_activity: Arc<AtomicBool>,
    pub(crate) finish_on_tools: Arc<std::collections::HashSet<String>>,
    pub(crate) finished_by_tool: Arc<AtomicBool>,
    pub(crate) root_trajectory_id: Arc<Mutex<Option<String>>>,
    pub(crate) turn_epoch: Arc<AtomicU64>,
    pub(crate) is_idle: Arc<AtomicBool>,
}

async fn emit_tool_result_event(
    tool_name: &str,
    call_id: &str,
    response_json: &str,
    error_msg: &str,
    active_writer: &Arc<tokio::sync::Mutex<Option<ChatResponseWriter>>>,
) {
    let guard = active_writer.lock().await;
    let Some(ref writer) = *guard else {
        return;
    };
    let tool_result = crate::types::ToolResult {
        name: tool_name.to_string(),
        id: Some(call_id.to_string()),
        result: parse_json_or_null(response_json, "emit_tool_result_event"),
        error: (!error_msg.is_empty()).then(|| error_msg.to_string()),
    };
    writer
        .emit_event(ResponseEvent::ToolResult(tool_result))
        .await;
}

async fn apply_finish_on_tool_completion(ctx: &ToolCallDispatchContext, dispatch_epoch: u64) {
    if ctx.turn_epoch.load(Ordering::SeqCst) != dispatch_epoch || ctx.is_idle.load(Ordering::SeqCst)
    {
        tracing::debug!(
            dispatch_epoch,
            "Skipping stale finish_on_tools halt for already-completed turn"
        );
        return;
    }
    ctx.finished_by_tool.store(true, Ordering::SeqCst);
    let halt_event = proto::localharness::InputEvent {
        event: Some(proto::localharness::input_event::Event::HaltRequest(true)),
    };
    if let Err(e) = ctx.event_tx.send(halt_event).await {
        tracing::debug!(error = %e, "Failed to send HaltRequest after finish_on_tools completion");
    }
}

pub(crate) async fn handle_tool_call(
    agent_id: AgentId,
    tool_call: proto::localharness::ToolCall,
    ctx: &ToolCallDispatchContext,
) {
    let dispatch_epoch = ctx.turn_epoch.load(Ordering::SeqCst);
    ctx.turn_activity.store(true, Ordering::SeqCst);
    let tool_name = tool_call.name.clone();
    let call_id = tool_call.id.clone();
    let args_json = if tool_call.arguments_json.trim().is_empty() {
        match tool_call.arguments.as_ref() {
            Some(st) => st.to_json_value().to_string(),
            None => String::new(),
        }
    } else {
        tool_call.arguments_json.clone()
    };
    let is_root_call =
        resolve_is_root_trajectory(&ctx.root_trajectory_id, &tool_call.trajectory_id, "", 0);

    emit_tool_call_stream_events(&tool_name, &call_id, &args_json, &ctx.active_writer).await;

    let (reg_opt, hr_opt, tool_state, surrogate_vault, conv_id) = match bridge_state().read() {
        Ok(state_guard) => {
            let bridge_entry = state_guard.get(&agent_id);
            (
                bridge_entry.and_then(|e| e.registry.clone()),
                bridge_entry.and_then(|e| e.hook_runner.clone()),
                bridge_entry.map_or_else(llm_tool::SharedState::new, |e| e.tool_state.clone()),
                bridge_entry
                    .map_or_else(llm_tool::SurrogateVault::new, |e| e.surrogate_vault.clone()),
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
            (
                None,
                None,
                llm_tool::SharedState::new(),
                llm_tool::SurrogateVault::new(),
                None,
            )
        }
    };

    let (response_json, error_msg) = if let Some(ref reg) = reg_opt {
        execute_custom_tool(CustomToolExecInputs {
            agent_id,
            tool_name: &tool_name,
            call_id: &call_id,
            args_json: &args_json,
            reg,
            hr_opt: hr_opt.as_ref(),
            tool_state,
            surrogate_vault,
            conv_id: conv_id.as_ref(),
        })
        .await
    } else {
        (String::new(), format!("Unknown custom tool {tool_name}"))
    };

    let should_finish =
        is_root_call && error_msg.is_empty() && ctx.finish_on_tools.contains(&tool_name);
    emit_tool_result_event(
        &tool_name,
        &call_id,
        &response_json,
        &error_msg,
        &ctx.active_writer,
    )
    .await;
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
    if let Err(e) = ctx.event_tx.send(resp_event).await {
        tracing::error!(error = %e, "Failed to send ToolResponse event");
    }
    if should_finish {
        apply_finish_on_tool_completion(ctx, dispatch_epoch).await;
    }
}

pub(crate) async fn handle_policy_decision_request(
    agent_id: AgentId,
    req: proto::localharness::PolicyDecisionRequest,
    event_tx: &mpsc::Sender<proto::localharness::InputEvent>,
) {
    let mut outcome = proto::localharness::PolicyEvaluationOutcome::Deny as i32;
    let mut deny_reason =
        "Denied: no AskUserHandler registered for interactive policy confirmation".to_string();

    let ph_opt = match bridge_state().read() {
        Ok(state_guard) => {
            if let Some(entry) = state_guard.get(&agent_id) {
                entry.policy_handler.clone()
            } else {
                deny_reason = format!("Denied: agent {agent_id} not found in bridge state");
                None
            }
        }
        Err(err) => {
            tracing::error!(error = %err, "Poisoned bridge_state read lock in handle_policy_decision_request");
            deny_reason =
                "Denied: bridge state lock poisoned during policy confirmation".to_string();
            None
        }
    };

    if let Some(ph) = ph_opt {
        let (tool_name, args_str) = match req.tool_args.as_ref() {
            Some(a) => (a.tool_name.clone(), a.arguments_json.as_str()),
            None => (String::new(), "{}"),
        };
        match super::tools::try_parse_and_normalize_tool_args(
            args_str,
            "handle_policy_decision_request",
        ) {
            Ok(normalized) => {
                let args_val = normalized.args;
                let confirmed_tool_name = tool_name.clone();
                let confirmed_args_val = args_val.clone();
                let allowed = tokio::task::spawn_blocking(move || {
                    ph.confirm(&tool_name, &args_val)
                })
                .await
                .unwrap_or_else(|e| {
                    tracing::error!(error = %e, "Policy confirmation panicked in spawn_blocking");
                    false
                });
                if allowed {
                    record_confirmed_tool_call(agent_id, &confirmed_tool_name, confirmed_args_val);
                    outcome = proto::localharness::PolicyEvaluationOutcome::Allow as i32;
                    deny_reason.clear();
                } else {
                    deny_reason = "Denied by user policy".to_string();
                }
            }
            Err(parse_err) => {
                tracing::warn!(error = %parse_err, "PolicyDecisionRequest rejected malformed tool arguments");
                deny_reason = format!("Denied: invalid tool arguments: {parse_err}");
            }
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
    turn_start_usage_clone: &Arc<Mutex<UsageMetadata>>,
    last_turn_usage_clone: &Arc<Mutex<UsageMetadata>>,
    active_writer_clone: &Arc<tokio::sync::Mutex<Option<ChatResponseWriter>>>,
) {
    if let Some(ref total) = usage.total {
        let cumulative = to_usage_metadata(total);
        match total_usage_clone.lock() {
            Ok(mut lock) => *lock = cumulative.clone(),
            Err(e) => {
                tracing::error!(error = %e, "Poisoned total_usage lock in handle_usage_update");
            }
        }
        let baseline = match turn_start_usage_clone.lock() {
            Ok(lock) => lock.clone(),
            Err(e) => {
                tracing::error!(error = %e, "Poisoned turn_start_usage lock in handle_usage_update");
                UsageMetadata::default()
            }
        };
        let turn_delta = cumulative.saturating_delta_since(&baseline);
        match last_turn_usage_clone.lock() {
            Ok(mut lock) => *lock = turn_delta.clone(),
            Err(e) => {
                tracing::error!(error = %e, "Poisoned last_turn_usage lock in handle_usage_update");
            }
        }
        let guard = active_writer_clone.lock().await;
        if let Some(ref writer) = *guard {
            writer.set_usage(turn_delta);
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
    pub(crate) finished_by_tool: &'a Arc<AtomicBool>,
    pub(crate) history: &'a Arc<Mutex<Vec<ConversationMessage>>>,
    pub(crate) last_response_text: &'a Arc<Mutex<Option<String>>>,
    pub(crate) root_trajectory_id: &'a Arc<Mutex<Option<String>>>,
}

pub(crate) async fn handle_trajectory_state_update(
    state_update: proto::localharness::TrajectoryStateUpdate,
    ctx: &TrajectoryStateContext<'_>,
) {
    let is_root = resolve_is_root_trajectory(
        ctx.root_trajectory_id,
        &state_update.trajectory_id,
        &state_update.parent_trajectory_id,
        state_update.depth,
    );
    if !is_root {
        if !state_update.error.is_empty() {
            tracing::debug!(
                trajectory_id = %state_update.trajectory_id,
                error = %state_update.error,
                "Subagent trajectory reported error; ignoring for root idle transition"
            );
        }
        return;
    }

    if matches!(
        state_update.state,
        TRAJECTORY_STATE_FULLY_IDLE..=TRAJECTORY_STATE_CANCELLED
    ) {
        ctx.turn_count.fetch_add(1, Ordering::SeqCst);
        ctx.is_idle.store(true, Ordering::SeqCst);
        ctx.idle_notify.notify_waiters();

        match ctx.last_response_text.lock() {
            Ok(lock) => {
                if let Some(ref text) = *lock
                    && !text.is_empty()
                {
                    match ctx.history.lock() {
                        Ok(mut hist) => {
                            hist.push(ConversationMessage {
                                role: MessageRole::Model,
                                content: text.clone(),
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

        let finished_by_tool = ctx.finished_by_tool.load(Ordering::SeqCst);
        let err_opt = if finished_by_tool || ctx.produced_output.load(Ordering::SeqCst) {
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

        let empty_completion_is_failure = !finished_by_tool
            && state_update.state == TRAJECTORY_STATE_FULLY_IDLE
            && !ctx.produced_output.load(Ordering::SeqCst)
            && !ctx.turn_activity.load(Ordering::SeqCst);

        let mut guard = ctx.active_writer.lock().await;
        if let Some(writer) = guard.take() {
            dispatch_terminal_trajectory_outcome(
                &writer,
                &state_update,
                finished_by_tool,
                err_opt,
                empty_completion_is_failure,
            )
            .await;
        }
    }
}

async fn dispatch_terminal_trajectory_outcome(
    writer: &crate::streaming::ChatResponseWriter,
    state_update: &proto::localharness::TrajectoryStateUpdate,
    finished_by_tool: bool,
    err_opt: Option<crate::streaming::StreamError>,
    empty_completion_is_failure: bool,
) {
    writer.set_stop_reason(StopReason::from_proto_i32(state_update.stop_reason));
    if finished_by_tool {
        return;
    }
    if state_update.state == TRAJECTORY_STATE_CANCELLED {
        let message = if state_update.error.is_empty() {
            "Turn cancelled".to_owned()
        } else {
            state_update.error.clone()
        };
        let code = if state_update.error_code.is_empty() {
            "CANCELLED".to_owned()
        } else {
            state_update.error_code.clone()
        };
        let http_code = match err_opt {
            Some(ref prev) => prev.http_code,
            None => crate::error::HTTP_CODE_UNKNOWN,
        };
        if let Err(e) = writer
            .send_error(crate::streaming::StreamError::with_codes(
                message,
                http_code,
                Some(code),
            ))
            .await
        {
            tracing::debug!(error = %e, "Failed to send trajectory cancellation error to writer");
        }
        return;
    }
    if !state_update.error.is_empty() {
        let http_code = match err_opt {
            Some(ref prev) => prev.http_code,
            None => crate::error::HTTP_CODE_UNKNOWN,
        };
        let code_opt =
            (!state_update.error_code.is_empty()).then(|| state_update.error_code.clone());
        if let Err(e) = writer
            .send_error(crate::streaming::StreamError::with_codes(
                &state_update.error,
                http_code,
                code_opt,
            ))
            .await
        {
            tracing::debug!(error = %e, "Failed to send trajectory error to writer");
        }
    } else if let Some(mut err) = err_opt {
        if err.error_code.is_none() && !state_update.error_code.is_empty() {
            err.error_code = Some(state_update.error_code.clone());
        }
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

#[cfg(test)]
mod tests;

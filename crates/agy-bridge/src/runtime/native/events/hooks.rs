//! Lifecycle hook dispatch (`CallHookRequest` → [`Hooks`]) for the native runtime.

use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};

use tokio::sync::mpsc;

use super::parse_json_or_null;
use crate::{
    agent::AgentId,
    hooks::{
        OnToolErrorContext, PostToolCallContext, PostTurnContext, PreToolCallDecideContext,
        PreTurnContext,
    },
    proto,
    runtime::bridge_state::bridge_state,
};

pub(super) fn dispatch_pre_turn_hook(
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

pub(super) fn dispatch_pre_tool_hook(
    hr: &crate::hooks::Hooks,
    req: &proto::localharness::CallHookRequest,
) -> proto::localharness::call_hook_response::Result {
    let (tool_name, args_val) = match req.args {
        Some(proto::localharness::call_hook_request::Args::PreToolArgs(ref a)) => {
            let parsed = parse_json_or_null(&a.arguments_json, "dispatch_pre_tool_hook");
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
                modified_args: None,
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
            modified_args: None,
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
    // `OnToolErrorContext` documents `Null` as "no tool arguments / metadata".
    let (tool_args, metadata) = match crate::runtime::bridge_state::take_last_tool_error(agent_id) {
        Some(captured) => {
            let metadata = match captured.metadata {
                Some(map) => serde_json::Value::Object(map),
                None => serde_json::Value::Null,
            };
            (captured.tool_args, metadata)
        }
        None => (serde_json::Value::Null, serde_json::Value::Null),
    };
    let ctx = OnToolErrorContext {
        tool_name,
        tool_args,
        error: error_message,
        metadata,
    };
    let res = hr.run_on_tool_error(&ctx);
    let mut err_result = proto::localharness::OnToolErrorResult::default();
    if let Some(msg) = res {
        err_result.custom_error_message = msg;
    }
    proto::localharness::call_hook_response::Result::OnToolErrorResult(err_result)
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

pub(super) fn dispatch_on_compaction_hook(
    hr: &crate::hooks::Hooks,
) -> proto::localharness::call_hook_response::Result {
    hr.run_on_compaction(&crate::hooks::OnCompactionContext {});
    proto::localharness::call_hook_response::Result::EmptyResult(
        proto::localharness::EmptyResult {},
    )
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
            8 => Some(dispatch_on_compaction_hook(&hr)),
            9 => Some(dispatch_stop_hook(&hr, &req)),
            _ => match req.args {
                Some(proto::localharness::call_hook_request::Args::OnCompactionArgs(_)) => {
                    Some(dispatch_on_compaction_hook(&hr))
                }
                Some(proto::localharness::call_hook_request::Args::StopArgs(_)) => {
                    Some(dispatch_stop_hook(&hr, &req))
                }
                _ => Some(
                    proto::localharness::call_hook_response::Result::EmptyResult(
                        proto::localharness::EmptyResult {},
                    ),
                ),
            },
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

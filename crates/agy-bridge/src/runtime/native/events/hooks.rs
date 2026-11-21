//! Lifecycle hook dispatch (`CallHookRequest` → [`Hooks`]) for the native runtime.

use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};

use tokio::sync::mpsc;

use crate::{
    agent::AgentId,
    hooks::{
        OnToolErrorContext, PostToolCallContext, PostTurnContext, PreToolCallDecideContext,
        PreTurnContext,
    },
    proto,
    runtime::bridge_state::{
        bridge_state, mark_pre_tool_evaluated, sanitize_with_agent_floor, take_tool_error_for_call,
        take_tool_output_for_call,
    },
};

pub(super) fn dispatch_pre_turn_hook(
    hr: &crate::hooks::Hooks,
    req: &proto::localharness::CallHookRequest,
    turn: u32,
) -> proto::localharness::call_hook_response::Result {
    let prompt = match req.args {
        Some(proto::localharness::call_hook_request::Args::PreTurn(ref a)) => {
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
    proto::localharness::call_hook_response::Result::PreTurn(proto::localharness::PreTurnResult {
        decision: if res.allow { 1 } else { 2 },
        reason: res.message,
    })
}

fn dispatch_post_turn_hook(
    hr: &crate::hooks::Hooks,
    req: &proto::localharness::CallHookRequest,
    turn: u32,
) -> proto::localharness::call_hook_response::Result {
    let response_text = match req.args {
        Some(proto::localharness::call_hook_request::Args::PostTurn(ref a)) => {
            a.response_text.clone()
        }
        _ => String::new(),
    };
    let ctx = PostTurnContext {
        response_text,
        turn_number: turn,
    };
    hr.run_post_turn(&ctx);
    proto::localharness::call_hook_response::Result::Empty(proto::localharness::EmptyResult {})
}

pub(super) fn dispatch_pre_tool_hook(
    agent_id: AgentId,
    hr: &crate::hooks::Hooks,
    req: &proto::localharness::CallHookRequest,
) -> proto::localharness::call_hook_response::Result {
    let (tool_name, call_id, args_res) = match req.args {
        Some(proto::localharness::call_hook_request::Args::PreTool(ref a)) => {
            let parsed = crate::runtime::native::tools::try_parse_and_normalize_tool_args(
                &a.arguments_json,
                "dispatch_pre_tool_hook",
            );
            (a.tool_name.clone(), a.call_id.clone(), parsed)
        }
        _ => (
            String::new(),
            String::new(),
            Ok(crate::runtime::native::tools::NormalizedToolPayload {
                args: serde_json::Value::Null,
                canonical_path: None,
            }),
        ),
    };
    let normalized = match args_res {
        Ok(payload) => payload,
        Err(reason) => {
            return proto::localharness::call_hook_response::Result::PreTool(
                proto::localharness::PreToolResult {
                    decision: 2,
                    reason: sanitize_with_agent_floor(agent_id, &reason),
                    modified_args: None,
                },
            );
        }
    };

    let initial_ctx = PreToolCallDecideContext::new(tool_name.clone(), normalized.args);
    let mut transformed = hr.run_transform_tool_input(&initial_ctx);
    if transformed != initial_ctx.tool_args {
        crate::runtime::bridge_state::invalidate_confirmed_tool_call(
            agent_id,
            &tool_name,
            &initial_ctx.tool_args,
        );
        if let Err(reason) =
            crate::runtime::native::tools::normalize_json_object_paths_in_place(&mut transformed)
        {
            return proto::localharness::call_hook_response::Result::PreTool(
                proto::localharness::PreToolResult {
                    decision: 2,
                    reason: sanitize_with_agent_floor(agent_id, &reason),
                    modified_args: None,
                },
            );
        }
    }

    let decide_ctx = PreToolCallDecideContext::new(tool_name, transformed.clone());
    let res = hr.run_pre_tool_call_decide(&decide_ctx);
    if !res.allow {
        return proto::localharness::call_hook_response::Result::PreTool(
            proto::localharness::PreToolResult {
                decision: 2,
                reason: sanitize_with_agent_floor(agent_id, &res.message),
                modified_args: None,
            },
        );
    }

    mark_pre_tool_evaluated(agent_id, &call_id);
    let modified_args = if transformed == initial_ctx.tool_args {
        None
    } else {
        transformed
            .as_object()
            .and_then(proto::genai::Struct::from_json_object)
    };
    proto::localharness::call_hook_response::Result::PreTool(proto::localharness::PreToolResult {
        decision: 1,
        reason: res.message,
        modified_args,
    })
}

fn dispatch_post_tool_hook(
    agent_id: AgentId,
    hr: &crate::hooks::Hooks,
    req: &proto::localharness::CallHookRequest,
) -> proto::localharness::call_hook_response::Result {
    let (tool_name, call_id, result) = match req.args {
        Some(proto::localharness::call_hook_request::Args::PostTool(ref a)) => {
            (a.tool_name.clone(), a.call_id.clone(), a.result.clone())
        }
        _ => (String::new(), String::new(), String::new()),
    };
    let (tool_args, metadata) = match take_tool_output_for_call(agent_id, &call_id) {
        Some(captured) => (captured.tool_args, captured.metadata),
        None => (serde_json::Value::Null, serde_json::Value::Null),
    };
    let ctx = PostToolCallContext {
        tool_name,
        tool_args,
        result,
        metadata,
    };
    hr.run_post_tool_call(&ctx);
    proto::localharness::call_hook_response::Result::Empty(proto::localharness::EmptyResult {})
}

fn dispatch_on_tool_error_hook(
    agent_id: AgentId,
    hr: &crate::hooks::Hooks,
    req: &proto::localharness::CallHookRequest,
) -> proto::localharness::call_hook_response::Result {
    let (tool_name, call_id, error_message) = match req.args {
        Some(proto::localharness::call_hook_request::Args::OnToolError(ref a)) => (
            a.tool_name.clone(),
            a.call_id.clone(),
            a.error_message.clone(),
        ),
        _ => (String::new(), String::new(), String::new()),
    };
    let (tool_args, metadata) = match take_tool_error_for_call(agent_id, &call_id) {
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
        err_result.custom_error_message = sanitize_with_agent_floor(agent_id, &msg);
    }
    proto::localharness::call_hook_response::Result::OnToolError(err_result)
}

fn dispatch_on_session_start_hook(
    agent_id: AgentId,
    hr: &crate::hooks::Hooks,
) -> proto::localharness::call_hook_response::Result {
    if crate::runtime::bridge_state::claim_session_start_once(agent_id) {
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
    }
    proto::localharness::call_hook_response::Result::Empty(proto::localharness::EmptyResult {})
}

fn dispatch_on_session_end_hook(
    agent_id: AgentId,
    hr: &crate::hooks::Hooks,
) -> proto::localharness::call_hook_response::Result {
    if crate::runtime::bridge_state::claim_session_end_once(agent_id) {
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
    }
    proto::localharness::call_hook_response::Result::Empty(proto::localharness::EmptyResult {})
}

fn parse_stop_args(a: &proto::localharness::StopArgs) -> crate::hooks::StopArgs {
    let stop_reason = crate::types::StopReason::from_proto_i32(a.stop_reason);
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
    proto::localharness::call_hook_response::Result::Empty(proto::localharness::EmptyResult {})
}

fn dispatch_stop_hook(
    hr: &crate::hooks::Hooks,
    req: &proto::localharness::CallHookRequest,
) -> proto::localharness::call_hook_response::Result {
    let stop_args = match req.args {
        Some(proto::localharness::call_hook_request::Args::Stop(ref a)) => parse_stop_args(a),
        _ => crate::hooks::StopArgs::default(),
    };
    let res = hr.run_stop(&stop_args);
    proto::localharness::call_hook_response::Result::Stop(proto::localharness::StopResult {
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
            5 => Some(dispatch_pre_tool_hook(agent_id, &hr, &req)),
            6 => Some(dispatch_post_tool_hook(agent_id, &hr, &req)),
            7 => Some(dispatch_on_tool_error_hook(agent_id, &hr, &req)),
            8 => Some(dispatch_on_compaction_hook(&hr)),
            9 => Some(dispatch_stop_hook(&hr, &req)),
            _ => match req.args {
                Some(proto::localharness::call_hook_request::Args::OnCompaction(_)) => {
                    Some(dispatch_on_compaction_hook(&hr))
                }
                Some(proto::localharness::call_hook_request::Args::Stop(_)) => {
                    Some(dispatch_stop_hook(&hr, &req))
                }
                _ => Some(proto::localharness::call_hook_response::Result::Empty(
                    proto::localharness::EmptyResult {},
                )),
            },
        })
        .await
        {
            Ok(res) => res,
            Err(e) => {
                tracing::error!(error = %e, "Hook execution panicked in spawn_blocking");
                Some(proto::localharness::call_hook_response::Result::Empty(
                    proto::localharness::EmptyResult {},
                ))
            }
        }
    } else {
        Some(proto::localharness::call_hook_response::Result::Empty(
            proto::localharness::EmptyResult {},
        ))
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

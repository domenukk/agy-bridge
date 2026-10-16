use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU32, Ordering},
};

use futures::{SinkExt, stream::StreamExt};
use tokio::{
    net::TcpStream,
    sync::{Notify, mpsc},
    time::sleep,
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{client::IntoClientRequest, protocol::Message},
};

use super::{
    NativeAgentSession, WS_CONNECT_RETRY_COUNT, WS_CONNECT_RETRY_DELAY,
    config::build_harness_config,
    events::{
        StepUpdateContext, TrajectoryStateContext, handle_call_hook_request,
        handle_policy_decision_request, handle_step_update, handle_tool_call,
        handle_trajectory_state_update, handle_usage_update, to_usage_metadata,
    },
    proto,
    session::InitialHarnessState,
};
use crate::{
    agent::AgentId,
    config::AgentConfig,
    error::Error,
    hooks::Hooks,
    policies::PolicySet,
    types::{ConversationMessage, MessageRole, UsageMetadata},
};

pub(super) type HarnessWsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

pub(super) async fn connect_to_harness(port: u16, api_key: &str) -> Result<HarnessWsStream, Error> {
    let ws_url = format!("ws://127.0.0.1:{port}/");
    let mut ws_stream_opt = None;
    let mut retry_count = 0;

    for _ in 0..WS_CONNECT_RETRY_COUNT {
        let mut req = ws_url
            .clone()
            .into_client_request()
            .map_err(|e| Error::ConnectionError {
                message: format!("Failed to create WebSocket request: {e}"),
            })?;

        if !api_key.is_empty() {
            req.headers_mut().insert(
                "x-goog-api-key",
                api_key.parse().map_err(|e| Error::ConnectionError {
                    message: format!("Invalid API key header: {e}"),
                })?,
            );
        }

        match connect_async(req).await {
            Ok((ws, _)) => {
                ws_stream_opt = Some(ws);
                break;
            }
            Err(e) => {
                tracing::debug!(
                    error = %e,
                    attempt = retry_count,
                    "Retrying WebSocket connection to harness"
                );
            }
        }
        retry_count += 1;
        sleep(WS_CONNECT_RETRY_DELAY).await;
    }

    ws_stream_opt.ok_or_else(|| Error::ConnectionError {
        message: format!(
            "Failed to connect to local harness WebSocket at {ws_url} after {retry_count} attempts"
        ),
    })
}

pub(super) async fn initialize_harness(
    ws: &mut HarnessWsStream,
    config: &AgentConfig,
    hook_runner: Option<&Arc<Hooks>>,
    policies: &PolicySet,
) -> Result<InitialHarnessState, Error> {
    let harness_config = build_harness_config(config, None, hook_runner, policies);
    let init_event = proto::localharness::InitializeConversationEvent {
        config: Some(harness_config),
    };
    let init_json = serde_json::to_string(&init_event).map_err(|e| Error::BackendError {
        message: format!("Failed to serialize InitializeConversationEvent: {e}"),
    })?;

    ws.send(Message::Text(init_json.into()))
        .await
        .map_err(|e| Error::BackendError {
            message: format!("Failed to send InitializeConversationEvent: {e}"),
        })?;

    let first_msg = ws
        .next()
        .await
        .ok_or_else(|| Error::BackendError {
            message: "WebSocket closed before InitializeConversationResponse".to_string(),
        })?
        .map_err(|e| Error::BackendError {
            message: format!("WebSocket read error: {e}"),
        })?;

    let first_text = first_msg.to_text().map_err(|e| Error::BackendError {
        message: format!("Expected text message for init response: {e}"),
    })?;

    let init_resp: proto::localharness::OutputEvent =
        serde_json::from_str(first_text).map_err(|e| Error::BackendError {
            message: format!("Failed to parse InitializeConversationResponse ({first_text}): {e}"),
        })?;

    Ok(parse_init_response(&init_resp, config))
}

fn parse_init_response(
    init_resp: &proto::localharness::OutputEvent,
    config: &AgentConfig,
) -> InitialHarnessState {
    let mut cascade_id = None;
    let mut usage = UsageMetadata::default();
    let mut sandbox_status = None;
    let mut history = Vec::new();
    let mut last_response_text = None;
    let mut compaction_indices = Vec::new();
    let mut turn_count = 0u32;

    if let Some(proto::localharness::output_event::Event::InitializeConversationResponse(
        ref resp,
    )) = init_resp.event
    {
        if !resp.cascade_id.is_empty() {
            cascade_id = Some(resp.cascade_id.clone());
        }
        if let Some(ref u) = resp.cumulative_usage {
            usage = to_usage_metadata(u);
        }
        if let Some(ref s) = resp.sandbox_status {
            sandbox_status = Some(crate::types::SandboxStatus {
                available: s.available,
                unavailable_reason: if s.unavailable_reason.is_empty() {
                    None
                } else {
                    Some(s.unavailable_reason.clone())
                },
            });
        }
        for step_update in &resp.history {
            if step_update.compaction.is_some()
                && !compaction_indices.contains(&step_update.step_index)
            {
                compaction_indices.push(step_update.step_index);
            }
            if step_update.source == 2 {
                let text = if step_update.request_text.is_empty() {
                    step_update.text.clone()
                } else {
                    step_update.request_text.clone()
                };
                if !text.is_empty() {
                    history.push(ConversationMessage {
                        role: MessageRole::User,
                        content: text,
                    });
                    turn_count = turn_count.saturating_add(1);
                }
            } else if (step_update.source == 3 || step_update.source == 0)
                && !step_update.text.is_empty()
            {
                history.push(ConversationMessage {
                    role: MessageRole::Model,
                    content: step_update.text.clone(),
                });
                last_response_text = Some(step_update.text.clone());
            }
        }
    }

    if history.is_empty() && !config.initial_history.is_empty() {
        history.clone_from(&config.initial_history);
        last_response_text = history
            .iter()
            .rev()
            .find(|m| m.role == MessageRole::Model)
            .map(|m| m.content.clone());
        let user_turns = history
            .iter()
            .filter(|m| m.role == MessageRole::User)
            .count();
        turn_count = match u32::try_from(user_turns) {
            Ok(count) => count,
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    user_turns,
                    "initial_history user turn count exceeds u32; saturating turn_count"
                );
                u32::MAX
            }
        };
    }

    InitialHarnessState {
        cascade_id,
        usage,
        sandbox_status,
        history,
        last_response_text,
        compaction_indices,
        turn_count,
    }
}

pub(super) fn remove_persisted_conversation_files(save_dir: &std::path::Path, cascade_id: &str) {
    if cascade_id.is_empty() {
        return;
    }
    for candidate in [
        save_dir.join(format!("{cascade_id}.db")),
        save_dir.join(format!("traj-{cascade_id}")),
        save_dir.join(cascade_id),
    ] {
        if candidate.is_dir() {
            if let Err(e) = std::fs::remove_dir_all(&candidate) {
                tracing::debug!(path = %candidate.display(), error = %e, "Failed removing persisted conversation dir");
            }
        } else if candidate.is_file()
            && let Err(e) = std::fs::remove_file(&candidate)
        {
            tracing::debug!(path = %candidate.display(), error = %e, "Failed removing persisted conversation file");
        }
    }
}

fn spawn_ws_sink_task(
    runtime: &tokio::runtime::Handle,
    mut ws_sink: futures::stream::SplitSink<HarnessWsStream, Message>,
    mut event_rx: mpsc::Receiver<proto::localharness::InputEvent>,
) {
    runtime.spawn(async move {
        while let Some(event) = event_rx.recv().await {
            let is_session_end = matches!(
                event.event,
                Some(proto::localharness::input_event::Event::SessionEndRequest(
                    true
                ))
            );
            match serde_json::to_string(&event) {
                Ok(json_str) => {
                    if let Err(e) = ws_sink.send(Message::Text(json_str.into())).await {
                        if is_session_end {
                            tracing::debug!(error = %e, "WebSocket already closed on SessionEndRequest");
                        } else {
                            tracing::error!(error = %e, "Failed to write InputEvent to WebSocket");
                        }
                        break;
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "Failed to serialize InputEvent");
                }
            }
            if is_session_end {
                if let Err(e) = ws_sink.close().await {
                    tracing::debug!(error = %e, "WebSocket close frame error on session end");
                }
                break;
            }
        }
    });
}

struct SessionReaderContext {
    runtime: tokio::runtime::Handle,
    active_writer: Arc<tokio::sync::Mutex<Option<crate::streaming::ChatResponseWriter>>>,
    last_response_text: Arc<Mutex<Option<String>>>,
    last_error: Arc<Mutex<Option<crate::streaming::StreamError>>>,
    produced_output: Arc<AtomicBool>,
    turn_activity: Arc<AtomicBool>,
    compaction_indices: Arc<Mutex<Vec<u32>>>,
    event_tx: mpsc::Sender<proto::localharness::InputEvent>,
    turn_count: Arc<AtomicU32>,
    total_usage: Arc<Mutex<crate::types::UsageMetadata>>,
    last_turn_usage: Arc<Mutex<crate::types::UsageMetadata>>,
    is_idle: Arc<AtomicBool>,
    idle_notify: Arc<Notify>,
    history: Arc<Mutex<Vec<ConversationMessage>>>,
}

impl SessionReaderContext {
    fn from_session(session: &NativeAgentSession, runtime: &tokio::runtime::Handle) -> Self {
        Self {
            runtime: runtime.clone(),
            active_writer: Arc::clone(&session.active_writer),
            last_response_text: Arc::clone(&session.last_response_text),
            last_error: Arc::clone(&session.last_error),
            produced_output: Arc::clone(&session.produced_output),
            turn_activity: Arc::clone(&session.turn_activity),
            compaction_indices: Arc::clone(&session.compaction_indices),
            event_tx: session.event_tx.clone(),
            turn_count: Arc::clone(&session.turn_count),
            total_usage: Arc::clone(&session.total_usage),
            last_turn_usage: Arc::clone(&session.last_turn_usage),
            is_idle: Arc::clone(&session.is_idle),
            idle_notify: Arc::clone(&session.idle_notify),
            history: Arc::clone(&session.history),
        }
    }
}

fn spawn_ws_reader_task(
    runtime: &tokio::runtime::Handle,
    agent_id: AgentId,
    mut ws_stream_reader: futures::stream::SplitStream<HarnessWsStream>,
    session: &NativeAgentSession,
) {
    let ctx = SessionReaderContext::from_session(session, runtime);
    let connected_clone = Arc::clone(&session.connected);

    runtime.spawn(async move {
        while let Some(msg_res) = ws_stream_reader.next().await {
            let msg = match msg_res {
                Ok(m) => m,
                Err(e) => {
                    if connected_clone.load(Ordering::Acquire) {
                        tracing::error!(error = %e, "Error reading from harness WebSocket");
                    } else {
                        tracing::debug!(
                            agent_id = %agent_id,
                            error = %e,
                            "Harness WebSocket closed during agent shutdown"
                        );
                    }
                    break;
                }
            };

            if msg.is_close() {
                tracing::debug!(agent_id = %agent_id, "Harness WebSocket closed gracefully");
                break;
            }

            let Ok(text) = msg.to_text() else { continue };

            let output_event: proto::localharness::OutputEvent = match serde_json::from_str(text) {
                Ok(ev) => ev,
                Err(e) => {
                    tracing::warn!(error = %e, raw = text, "Failed to deserialize OutputEvent");
                    continue;
                }
            };

            if let Some(event) = output_event.event {
                dispatch_output_event(agent_id, event, &ctx).await;
            }
        }

        handle_ws_reader_exit(
            &connected_clone,
            &ctx.is_idle,
            &ctx.idle_notify,
            &ctx.active_writer,
        )
        .await;
    });
}

async fn dispatch_output_event(
    agent_id: AgentId,
    event: proto::localharness::output_event::Event,
    ctx: &SessionReaderContext,
) {
    match event {
        proto::localharness::output_event::Event::StepUpdate(step_update) => {
            let step_ctx = StepUpdateContext {
                active_writer: &ctx.active_writer,
                last_response_text: &ctx.last_response_text,
                last_error: &ctx.last_error,
                produced_output: &ctx.produced_output,
                turn_activity: &ctx.turn_activity,
                compaction_indices: &ctx.compaction_indices,
            };
            handle_step_update(agent_id, step_update, &step_ctx).await;
        }
        proto::localharness::output_event::Event::ToolCall(tool_call) => {
            let writer = Arc::clone(&ctx.active_writer);
            let tx = ctx.event_tx.clone();
            let activity = Arc::clone(&ctx.turn_activity);
            ctx.runtime.spawn(async move {
                handle_tool_call(agent_id, tool_call, &writer, &tx, &activity).await;
            });
        }
        proto::localharness::output_event::Event::CallHookRequest(req) => {
            let tc = Arc::clone(&ctx.turn_count);
            let tx = ctx.event_tx.clone();
            ctx.runtime.spawn(async move {
                handle_call_hook_request(agent_id, req, &tc, &tx).await;
            });
        }
        proto::localharness::output_event::Event::PolicyDecisionRequest(req) => {
            let tx = ctx.event_tx.clone();
            ctx.runtime.spawn(async move {
                handle_policy_decision_request(agent_id, req, &tx).await;
            });
        }
        proto::localharness::output_event::Event::UsageUpdate(usage) => {
            handle_usage_update(
                usage,
                &ctx.total_usage,
                &ctx.last_turn_usage,
                &ctx.active_writer,
            )
            .await;
        }
        proto::localharness::output_event::Event::TrajectoryStateUpdate(state_update) => {
            let traj_ctx = TrajectoryStateContext {
                turn_count: &ctx.turn_count,
                is_idle: &ctx.is_idle,
                idle_notify: &ctx.idle_notify,
                active_writer: &ctx.active_writer,
                last_error: &ctx.last_error,
                produced_output: &ctx.produced_output,
                turn_activity: &ctx.turn_activity,
                history: &ctx.history,
                last_response_text: &ctx.last_response_text,
            };
            handle_trajectory_state_update(state_update, &traj_ctx).await;
        }
        _ => {}
    }
}

/// Clean up session state after the harness WebSocket reader loop terminates.
///
/// Marks the session disconnected and, if a turn was still active, surfaces a
/// disconnect error to its writer so the consumer observes a failure rather
/// than a misleading empty `Ok` response.
async fn handle_ws_reader_exit(
    connected: &Arc<AtomicBool>,
    is_idle: &Arc<AtomicBool>,
    idle_notify: &Arc<Notify>,
    active_writer: &Arc<tokio::sync::Mutex<Option<crate::streaming::ChatResponseWriter>>>,
) {
    connected.store(false, Ordering::SeqCst);
    is_idle.store(true, Ordering::SeqCst);
    idle_notify.notify_waiters();
    let mut writer_guard = active_writer.lock().await;
    if let Some(writer) = writer_guard.take() {
        // The socket closed while a turn was still active. Report it as an
        // error so the consumer sees a failure rather than an empty `Ok`.
        if let Err(e) = writer
            .send_error(crate::streaming::StreamError::new(
                "Harness WebSocket closed before the turn completed; \
                 the agent session has terminated",
            ))
            .await
        {
            tracing::debug!(error = %e, "Failed to send disconnect error to active writer");
        }
    }
}

pub(super) fn spawn_session_io_tasks(
    runtime: &tokio::runtime::Handle,
    agent_id: AgentId,
    ws: HarnessWsStream,
    session: &NativeAgentSession,
    event_rx: mpsc::Receiver<proto::localharness::InputEvent>,
) {
    let (ws_sink, ws_stream_reader) = ws.split();
    spawn_ws_sink_task(runtime, ws_sink, event_rx);
    spawn_ws_reader_task(runtime, agent_id, ws_stream_reader, session);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming::channel_with_buffer;

    #[tokio::test]
    async fn handle_ws_reader_exit_marks_disconnected_and_fails_active_turn() {
        let connected = Arc::new(AtomicBool::new(true));
        let is_idle = Arc::new(AtomicBool::new(false));
        let idle_notify = Arc::new(Notify::new());
        let (writer, handle) = channel_with_buffer(8);
        let active_writer = Arc::new(tokio::sync::Mutex::new(Some(writer)));

        handle_ws_reader_exit(&connected, &is_idle, &idle_notify, &active_writer).await;

        assert!(!connected.load(Ordering::SeqCst));
        assert!(is_idle.load(Ordering::SeqCst));
        assert!(active_writer.lock().await.is_none());

        let err = handle
            .text()
            .await
            .expect_err("active turn should fail with StreamError on WebSocket exit");
        assert!(
            err.to_string()
                .contains("Harness WebSocket closed before the turn completed"),
            "unexpected error: {err}"
        );
    }
}

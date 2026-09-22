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
        TrajectoryStateContext, handle_call_hook_request, handle_policy_decision_request,
        handle_step_update, handle_tool_call, handle_trajectory_state_update, handle_usage_update,
        to_usage_metadata,
    },
    proto,
};
use crate::{
    agent::AgentId,
    config::AgentConfig,
    error::Error,
    hooks::Hooks,
    policies::PolicySet,
    types::{ConversationMessage, UsageMetadata},
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
) -> Result<
    (
        Option<String>,
        UsageMetadata,
        Option<crate::types::SandboxStatus>,
    ),
    Error,
> {
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

    let mut initial_cascade_id = None;
    let mut initial_usage = UsageMetadata::default();
    let mut initial_sandbox_status = None;

    if let Some(proto::localharness::output_event::Event::InitializeConversationResponse(
        ref resp,
    )) = init_resp.event
    {
        if !resp.cascade_id.is_empty() {
            initial_cascade_id = Some(resp.cascade_id.clone());
        }
        if let Some(ref u) = resp.cumulative_usage {
            initial_usage = to_usage_metadata(u);
        }
        if let Some(ref s) = resp.sandbox_status {
            initial_sandbox_status = Some(crate::types::SandboxStatus {
                available: s.available,
                unavailable_reason: if s.unavailable_reason.is_empty() {
                    None
                } else {
                    Some(s.unavailable_reason.clone())
                },
            });
        }
    }

    Ok((initial_cascade_id, initial_usage, initial_sandbox_status))
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
                        tracing::error!(error = %e, "Failed to write InputEvent to WebSocket");
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
                    tracing::error!(error = %e, "Error reading from harness WebSocket");
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
            handle_step_update(
                agent_id,
                step_update,
                &ctx.active_writer,
                &ctx.last_response_text,
                &ctx.last_error,
                &ctx.produced_output,
                &ctx.turn_activity,
            )
            .await;
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

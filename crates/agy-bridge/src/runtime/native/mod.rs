//! Native local harness runtime: communicates with the `localharness` binary
//! via stdio protobuf handshake and WebSocket JSON protocol without Python.

pub(crate) mod config;
pub(crate) mod events;
pub(crate) mod process;
pub(crate) mod session;
pub(crate) mod tools;

use std::{
    collections::HashMap,
    future::Future,
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::Duration,
};

use futures::{SinkExt, StreamExt};
use tokio::{
    net::TcpStream,
    sync::{Notify, mpsc},
    time::sleep,
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};

use self::{
    config::build_harness_config,
    events::{
        TrajectoryStateContext, handle_call_hook_request, handle_policy_decision_request,
        handle_step_update, handle_tool_call, handle_trajectory_state_update, handle_usage_update,
        to_proto_user_input, to_usage_metadata,
    },
    process::HarnessProcess,
    session::NativeAgentSession,
    tools::build_available_tools,
};
use crate::{
    agent::{AgentId, Runtime},
    config::AgentConfig,
    content::Content,
    error::Error,
    hooks::{Hooks, OnSessionEndContext, OnSessionStartContext, SessionContext},
    policies::PolicySet,
    proto,
    runtime::{bridge_state::bridge_state, config::RuntimeConfig},
    streaming::{ChatResponseHandle, channel_with_buffer},
    tools::AvailableTool,
    types::{ConversationMessage, MessageRole, UsageMetadata},
};

const WS_CONNECT_RETRY_COUNT: usize = 10;
const WS_CONNECT_RETRY_DELAY: Duration = Duration::from_millis(50);
const EVENT_CHANNEL_BUFFER_SIZE: usize = 64;

/// Native local harness runtime communicating natively via WebSocket and Protobuf.
pub struct NativeRuntime {
    sessions: Arc<RwLock<HashMap<AgentId, Arc<NativeAgentSession>>>>,
    config: RuntimeConfig,
}

impl NativeRuntime {
    /// Create a new native runtime instance.
    #[must_use]
    pub fn new(config: RuntimeConfig) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }

    /// Returns a reference to the runtime configuration.
    #[must_use]
    pub fn config(&self) -> &RuntimeConfig {
        &self.config
    }

    /// Return the number of agents currently live in this runtime.
    ///
    /// # Errors
    ///
    /// Returns an error if the internal session lock is poisoned.
    // NOLINT: async is required for interface compatibility with Python backend
    #[allow(unknown_lints, clippy::unused_async, clippy::unused_async_trait_impl)]
    pub async fn active_agent_count(&self) -> Result<usize, Error> {
        let sessions = self.sessions.read().map_err(|e| Error::BackendError {
            message: format!("Poisoned NATIVE_SESSIONS lock: {e}"),
        })?;
        Ok(sessions.len())
    }
}

impl Default for NativeRuntime {
    fn default() -> Self {
        Self::new(RuntimeConfig::default())
    }
}

async fn connect_to_harness(
    port: u16,
    api_key: &str,
) -> Result<WebSocketStream<MaybeTlsStream<TcpStream>>, Error> {
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
                tracing::debug!(error = %e, attempt = retry_count, "Retrying WebSocket connection to harness");
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

async fn initialize_harness(
    ws: &mut WebSocketStream<MaybeTlsStream<TcpStream>>,
    config: &AgentConfig,
    hook_runner: Option<&Arc<Hooks>>,
    policies: &PolicySet,
) -> Result<(Option<String>, UsageMetadata), Error> {
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
    }

    Ok((initial_cascade_id, initial_usage))
}

fn spawn_ws_sink_task(
    mut ws_sink: futures::stream::SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>,
    mut event_rx: mpsc::Receiver<proto::localharness::InputEvent>,
) {
    tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
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
        }
    });
}

fn spawn_ws_reader_task(
    agent_id: AgentId,
    mut ws_stream_reader: futures::stream::SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
    session: &NativeAgentSession,
) {
    let total_usage_clone = session.total_usage.clone();
    let last_turn_usage_clone = session.last_turn_usage.clone();
    let last_response_text_clone = session.last_response_text.clone();
    let turn_count_clone = session.turn_count.clone();
    let is_idle_clone = session.is_idle.clone();
    let idle_notify_clone = session.idle_notify.clone();
    let active_writer_clone = session.active_writer.clone();
    let event_tx_clone = session.event_tx.clone();
    let last_error_clone = session.last_error.clone();
    let produced_output_clone = session.produced_output.clone();
    let turn_activity_clone = session.turn_activity.clone();
    let connected_clone = session.connected.clone();
    let history_clone = session.history.clone();

    tokio::spawn(async move {
        while let Some(msg_res) = ws_stream_reader.next().await {
            let msg = match msg_res {
                Ok(m) => m,
                Err(e) => {
                    tracing::error!(error = %e, "Error reading from harness WebSocket");
                    break;
                }
            };

            let Ok(text) = msg.to_text() else { continue };

            let output_event: proto::localharness::OutputEvent = match serde_json::from_str(text) {
                Ok(ev) => ev,
                Err(e) => {
                    tracing::warn!(error = %e, raw = text, "Failed to deserialize OutputEvent");
                    continue;
                }
            };

            if let Some(event) = output_event.event {
                match event {
                    proto::localharness::output_event::Event::StepUpdate(step_update) => {
                        handle_step_update(
                            agent_id,
                            step_update,
                            &active_writer_clone,
                            &last_response_text_clone,
                            &last_error_clone,
                            &produced_output_clone,
                            &turn_activity_clone,
                        )
                        .await;
                    }
                    proto::localharness::output_event::Event::ToolCall(tool_call) => {
                        handle_tool_call(
                            agent_id,
                            tool_call,
                            &active_writer_clone,
                            &event_tx_clone,
                            &turn_activity_clone,
                        )
                        .await;
                    }
                    proto::localharness::output_event::Event::CallHookRequest(req) => {
                        handle_call_hook_request(agent_id, req, &turn_count_clone, &event_tx_clone)
                            .await;
                    }
                    proto::localharness::output_event::Event::PolicyDecisionRequest(req) => {
                        handle_policy_decision_request(agent_id, req, &event_tx_clone).await;
                    }
                    proto::localharness::output_event::Event::UsageUpdate(usage) => {
                        handle_usage_update(
                            usage,
                            &total_usage_clone,
                            &last_turn_usage_clone,
                            &active_writer_clone,
                        )
                        .await;
                    }
                    proto::localharness::output_event::Event::TrajectoryStateUpdate(
                        state_update,
                    ) => {
                        let ctx = TrajectoryStateContext {
                            turn_count: &turn_count_clone,
                            is_idle: &is_idle_clone,
                            idle_notify: &idle_notify_clone,
                            active_writer: &active_writer_clone,
                            last_error: &last_error_clone,
                            produced_output: &produced_output_clone,
                            turn_activity: &turn_activity_clone,
                            history: &history_clone,
                            last_response_text: &last_response_text_clone,
                        };
                        handle_trajectory_state_update(state_update, &ctx).await;
                    }
                    _ => {}
                }
            }
        }

        handle_ws_reader_exit(
            &connected_clone,
            &is_idle_clone,
            &idle_notify_clone,
            &active_writer_clone,
        )
        .await;
    });
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

fn spawn_session_io_tasks(
    agent_id: AgentId,
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    session: &NativeAgentSession,
    event_rx: mpsc::Receiver<proto::localharness::InputEvent>,
) {
    let (ws_sink, ws_stream_reader) = ws.split();
    spawn_ws_sink_task(ws_sink, event_rx);
    spawn_ws_reader_task(agent_id, ws_stream_reader, session);
}

fn extract_hook_runner_and_conv_id(agent_id: AgentId) -> (Option<Arc<Hooks>>, Option<String>) {
    match bridge_state().write() {
        Ok(mut map) => {
            let entry = map.remove(&agent_id);
            let hr = entry.as_ref().and_then(|e| e.hook_runner.clone());
            let conv_id = entry.and_then(|e| match e.conversation_id.lock() {
                Ok(guard) => guard.clone(),
                Err(err) => {
                    tracing::error!(
                        agent_id,
                        error = %err,
                        "conversation_id mutex poisoned during shutdown"
                    );
                    None
                }
            });
            (hr, conv_id)
        }
        Err(err) => {
            tracing::error!(
                agent_id,
                error = %err,
                "Poisoned bridge_state write lock on shutdown"
            );
            (None, None)
        }
    }
}

impl Runtime for NativeRuntime {
    fn create_agent(
        &self,
        agent_id: u64,
        config: AgentConfig,
    ) -> impl Future<Output = Result<(AgentId, Vec<AvailableTool>), Error>> + Send {
        let sessions_map = Arc::clone(&self.sessions);
        let custom_binary_path = self.config.harness_binary_path.clone();

        async move {
            if let Some(ref caps) = config.capabilities {
                caps.validate().map_err(|msg| Error::InvalidConfig {
                    message: msg.to_string(),
                })?;
            }
            if let Some(ref schema) = config.response_schema {
                schema.validate().map_err(|msg| Error::InvalidConfig {
                    message: msg.to_string(),
                })?;
            }

            let hook_runner = {
                let init_hooks = crate::runtime::initializing_hook_runners();
                match init_hooks.read() {
                    Ok(guard) => guard.get(&agent_id).cloned(),
                    Err(e) => {
                        tracing::error!(error = %e, "Poisoned initializing_hook_runners lock");
                        None
                    }
                }
            };
            let policies = PolicySet::validated_from(config.policies.clone())?;

            if let Some(ref hooks) = hook_runner {
                let session_id = config
                    .conversation_id
                    .clone()
                    .unwrap_or_else(|| format!("agent-{agent_id}"));
                let ctx = OnSessionStartContext {
                    session: SessionContext {
                        session_id,
                        agent_id,
                        started_at: std::time::SystemTime::now(),
                    },
                };
                hooks.run_on_session_start(&ctx);
            }

            let save_dir = config.save_dir.clone().unwrap_or_else(std::env::temp_dir);
            let process =
                HarnessProcess::spawn(&save_dir, None, custom_binary_path.as_deref()).await?;
            let port = process.port;
            let api_key = process.api_key.clone();

            let mut ws = connect_to_harness(port, &api_key).await?;
            let (initial_cascade_id, initial_usage) =
                initialize_harness(&mut ws, &config, hook_runner.as_ref(), &policies).await?;

            if let Some(ref id) = initial_cascade_id
                && let Err(e) =
                    crate::runtime::bridge_state::set_agent_conversation_id(agent_id, id.clone())
            {
                tracing::warn!(error = %e, "Failed to set agent conversation id on bridge_state");
            }

            let available_tools = build_available_tools(&config);
            let (event_tx, event_rx) =
                mpsc::channel::<proto::localharness::InputEvent>(EVENT_CHANNEL_BUFFER_SIZE);

            let session = Arc::new(NativeAgentSession {
                event_tx,
                history: Arc::new(Mutex::new(Vec::new())),
                total_usage: Arc::new(Mutex::new(initial_usage)),
                last_turn_usage: Arc::new(Mutex::new(UsageMetadata::default())),
                last_response_text: Arc::new(Mutex::new(None)),
                compaction_indices: Arc::new(Mutex::new(Vec::new())),
                turn_count: Arc::new(AtomicU32::new(0)),
                is_idle: Arc::new(AtomicBool::new(true)),
                idle_notify: Arc::new(Notify::new()),
                wakeup_notify: Arc::new(Notify::new()),
                active_writer: Arc::new(tokio::sync::Mutex::new(None)),
                last_error: Arc::new(Mutex::new(None)),
                produced_output: Arc::new(AtomicBool::new(false)),
                turn_activity: Arc::new(AtomicBool::new(false)),
                connected: Arc::new(AtomicBool::new(true)),
                process: tokio::sync::Mutex::new(Some(process)),
            });

            spawn_session_io_tasks(agent_id, ws, &session, event_rx);

            let mut sessions = sessions_map.write().map_err(|e| Error::BackendError {
                message: format!("Poisoned NATIVE_SESSIONS lock: {e}"),
            })?;
            sessions.insert(agent_id, session);

            Ok((agent_id, available_tools))
        }
    }

    fn chat(
        &self,
        agent_id: AgentId,
        content: &Content,
    ) -> impl Future<Output = Result<ChatResponseHandle, Error>> + Send {
        let sessions_map = Arc::clone(&self.sessions);
        let content = content.clone();

        async move {
            let session = {
                let sessions = sessions_map.read().map_err(|e| Error::BackendError {
                    message: format!("Poisoned NATIVE_SESSIONS lock: {e}"),
                })?;
                sessions
                    .get(&agent_id)
                    .cloned()
                    .ok_or(Error::AgentNotStarted)?
            };

            if !session.connected.load(Ordering::SeqCst) {
                return Err(Error::BackendError {
                    message: "Agent session is disconnected: the harness                               WebSocket has closed and cannot accept new turns"
                        .to_string(),
                });
            }

            session.is_idle.store(false, Ordering::SeqCst);
            session.produced_output.store(false, Ordering::SeqCst);
            session.turn_activity.store(false, Ordering::SeqCst);
            match session.last_error.lock() {
                Ok(mut err_lock) => {
                    *err_lock = None;
                }
                Err(e) => {
                    tracing::error!(error = %e, "Poisoned last_error lock in chat");
                }
            }

            let prompt_text = match &content {
                Content::Text { text } => text.clone(),
                _ => format!("{content:?}"),
            };

            match session.history.lock() {
                Ok(mut hist) => hist.push(ConversationMessage {
                    role: MessageRole::User,
                    content: prompt_text,
                }),
                Err(e) => {
                    tracing::error!(error = %e, "Poisoned history lock in chat");
                }
            }

            let (writer, handle) = channel_with_buffer(EVENT_CHANNEL_BUFFER_SIZE);
            {
                let mut active = session.active_writer.lock().await;
                *active = Some(writer);
            }

            let user_input = to_proto_user_input(&content);
            let input_event = proto::localharness::InputEvent {
                event: Some(proto::localharness::input_event::Event::UserInput(
                    user_input,
                )),
            };

            session
                .event_tx
                .send(input_event)
                .await
                .map_err(|e| Error::BackendError {
                    message: format!("Failed to send chat input event: {e}"),
                })?;

            Ok(handle)
        }
    }

    fn shutdown_agent(&self, agent_id: AgentId) -> impl Future<Output = Result<(), Error>> + Send {
        let sessions_map = Arc::clone(&self.sessions);
        async move {
            let session = {
                let mut sessions = sessions_map.write().map_err(|e| Error::BackendError {
                    message: format!("Poisoned NATIVE_SESSIONS lock: {e}"),
                })?;
                sessions.remove(&agent_id)
            };

            if let Some(s) = session {
                if let Err(e) = s
                    .event_tx
                    .send(proto::localharness::InputEvent {
                        event: Some(proto::localharness::input_event::Event::SessionEndRequest(
                            true,
                        )),
                    })
                    .await
                {
                    tracing::debug!(error = %e, "Harness already disconnected on shutdown");
                }

                let mut proc_opt = s.process.lock().await;
                if let Some(mut proc) = proc_opt.take() {
                    if let Err(e) = proc.child.kill().await {
                        tracing::debug!(error = %e, "Process already terminated on shutdown");
                    }
                    if let Err(e) = proc.child.wait().await {
                        tracing::debug!(error = %e, "Error waiting for process on shutdown");
                    }
                }

                let (hr_opt, conv_id) = extract_hook_runner_and_conv_id(agent_id);
                if let Some(hr) = hr_opt {
                    let session_id = conv_id.unwrap_or_else(|| format!("agent-{agent_id}"));
                    let ctx = OnSessionEndContext {
                        session: SessionContext {
                            session_id,
                            agent_id,
                            started_at: std::time::SystemTime::now(),
                        },
                    };
                    hr.run_on_session_end(&ctx);
                }
            }

            Ok(())
        }
    }

    fn try_shutdown_agent(&self, agent_id: AgentId) {
        match self.sessions.write() {
            Ok(mut sessions) => {
                if let Some(s) = sessions.remove(&agent_id) {
                    if let Err(e) = s.event_tx.try_send(proto::localharness::InputEvent {
                        event: Some(proto::localharness::input_event::Event::SessionEndRequest(
                            true,
                        )),
                    }) {
                        tracing::debug!(error = %e, "Harness channel closed on try_shutdown");
                    }

                    match s.process.try_lock() {
                        Ok(mut proc_opt) => {
                            if let Some(mut proc) = proc_opt.take() {
                                if let Err(e) = proc.child.start_kill() {
                                    tracing::debug!(error = %e, "Process already killed on try_shutdown");
                                }
                                match tokio::runtime::Handle::try_current() {
                                    Ok(handle) => {
                                        handle.spawn(async move {
                                            match proc.child.wait().await {
                                                Ok(status) => {
                                                    tracing::debug!(?status, "Child process reaped in try_shutdown");
                                                }
                                                Err(e) => {
                                                    tracing::debug!(error = %e, "Error reaping child process in try_shutdown");
                                                }
                                            }
                                        });
                                    }
                                    Err(e) => {
                                        tracing::debug!(error = %e, "No active tokio runtime to spawn child reap task");
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "Process lock contention on try_shutdown");
                        }
                    }

                    let (hr_opt, conv_id) = extract_hook_runner_and_conv_id(agent_id);
                    if let Some(hr) = hr_opt {
                        let session_id = conv_id.unwrap_or_else(|| format!("agent-{agent_id}"));
                        let ctx = OnSessionEndContext {
                            session: SessionContext {
                                session_id,
                                agent_id,
                                started_at: std::time::SystemTime::now(),
                            },
                        };
                        hr.run_on_session_end(&ctx);
                    }
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "Poisoned NATIVE_SESSIONS lock in try_shutdown_agent");
            }
        }
    }

    fn cancel(&self, agent_id: AgentId) -> impl Future<Output = Result<(), Error>> + Send {
        let sessions_map = Arc::clone(&self.sessions);
        async move {
            let session = {
                let sessions = sessions_map.read().map_err(|e| Error::BackendError {
                    message: format!("Poisoned NATIVE_SESSIONS lock: {e}"),
                })?;
                sessions
                    .get(&agent_id)
                    .cloned()
                    .ok_or(Error::AgentNotStarted)?
            };

            session
                .event_tx
                .send(proto::localharness::InputEvent {
                    event: Some(proto::localharness::input_event::Event::HaltRequest(true)),
                })
                .await
                .map_err(|e| Error::BackendError {
                    message: format!("Failed to send cancel halt request: {e}"),
                })?;

            Ok(())
        }
    }

    fn wait_for_idle(&self, agent_id: AgentId) -> impl Future<Output = Result<(), Error>> + Send {
        let sessions_map = Arc::clone(&self.sessions);
        async move {
            let session = {
                let sessions = sessions_map.read().map_err(|e| Error::BackendError {
                    message: format!("Poisoned NATIVE_SESSIONS lock: {e}"),
                })?;
                sessions
                    .get(&agent_id)
                    .cloned()
                    .ok_or(Error::AgentNotStarted)?
            };

            while !session.is_idle.load(Ordering::SeqCst) {
                session.idle_notify.notified().await;
            }

            Ok(())
        }
    }

    fn send(
        &self,
        agent_id: AgentId,
        content: &Content,
    ) -> impl Future<Output = Result<(), Error>> + Send {
        let chat_fut = self.chat(agent_id, content);
        async move {
            let _handle = chat_fut.await?;
            Ok(())
        }
    }

    fn signal_idle(&self, agent_id: AgentId) -> impl Future<Output = Result<(), Error>> + Send {
        let sessions_map = Arc::clone(&self.sessions);
        async move {
            let session = {
                let sessions = sessions_map.read().map_err(|e| Error::BackendError {
                    message: format!("Poisoned NATIVE_SESSIONS lock: {e}"),
                })?;
                sessions
                    .get(&agent_id)
                    .cloned()
                    .ok_or(Error::AgentNotStarted)?
            };

            session.is_idle.store(true, Ordering::SeqCst);
            session.idle_notify.notify_waiters();
            Ok(())
        }
    }

    fn wait_for_wakeup(
        &self,
        agent_id: AgentId,
        timeout: Duration,
    ) -> impl Future<Output = Result<bool, Error>> + Send {
        let sessions_map = Arc::clone(&self.sessions);
        async move {
            let session = {
                let sessions = sessions_map.read().map_err(|e| Error::BackendError {
                    message: format!("Poisoned NATIVE_SESSIONS lock: {e}"),
                })?;
                sessions
                    .get(&agent_id)
                    .cloned()
                    .ok_or(Error::AgentNotStarted)?
            };

            tokio::select! {
                () = session.wakeup_notify.notified() => Ok(true),
                () = tokio::time::sleep(timeout) => Ok(false),
            }
        }
    }

    fn history(
        &self,
        agent_id: AgentId,
    ) -> impl Future<Output = Result<Vec<ConversationMessage>, Error>> + Send {
        let sessions_map = Arc::clone(&self.sessions);
        async move {
            let session = {
                let sessions = sessions_map.read().map_err(|e| Error::BackendError {
                    message: format!("Poisoned NATIVE_SESSIONS lock: {e}"),
                })?;
                sessions
                    .get(&agent_id)
                    .cloned()
                    .ok_or(Error::AgentNotStarted)?
            };

            let hist = session.history.lock().map_err(|e| Error::BackendError {
                message: format!("Poisoned history lock: {e}"),
            })?;
            Ok(hist.clone())
        }
    }

    fn turn_count(&self, agent_id: AgentId) -> impl Future<Output = Result<u32, Error>> + Send {
        let sessions_map = Arc::clone(&self.sessions);
        async move {
            let session = {
                let sessions = sessions_map.read().map_err(|e| Error::BackendError {
                    message: format!("Poisoned NATIVE_SESSIONS lock: {e}"),
                })?;
                sessions
                    .get(&agent_id)
                    .cloned()
                    .ok_or(Error::AgentNotStarted)?
            };

            Ok(session.turn_count.load(Ordering::Relaxed))
        }
    }

    fn total_usage(
        &self,
        agent_id: AgentId,
    ) -> impl Future<Output = Result<UsageMetadata, Error>> + Send {
        let sessions_map = Arc::clone(&self.sessions);
        async move {
            let session = {
                let sessions = sessions_map.read().map_err(|e| Error::BackendError {
                    message: format!("Poisoned NATIVE_SESSIONS lock: {e}"),
                })?;
                sessions
                    .get(&agent_id)
                    .cloned()
                    .ok_or(Error::AgentNotStarted)?
            };

            let usage = session
                .total_usage
                .lock()
                .map_err(|e| Error::BackendError {
                    message: format!("Poisoned total_usage lock: {e}"),
                })?;
            Ok(usage.clone())
        }
    }

    fn last_turn_usage(
        &self,
        agent_id: AgentId,
    ) -> impl Future<Output = Result<UsageMetadata, Error>> + Send {
        let sessions_map = Arc::clone(&self.sessions);
        async move {
            let session = {
                let sessions = sessions_map.read().map_err(|e| Error::BackendError {
                    message: format!("Poisoned NATIVE_SESSIONS lock: {e}"),
                })?;
                sessions
                    .get(&agent_id)
                    .cloned()
                    .ok_or(Error::AgentNotStarted)?
            };

            let usage = session
                .last_turn_usage
                .lock()
                .map_err(|e| Error::BackendError {
                    message: format!("Poisoned last_turn_usage lock: {e}"),
                })?;
            Ok(usage.clone())
        }
    }

    fn clear_history(&self, agent_id: AgentId) -> impl Future<Output = Result<(), Error>> + Send {
        let sessions_map = Arc::clone(&self.sessions);
        async move {
            let session = {
                let sessions = sessions_map.read().map_err(|e| Error::BackendError {
                    message: format!("Poisoned NATIVE_SESSIONS lock: {e}"),
                })?;
                sessions
                    .get(&agent_id)
                    .cloned()
                    .ok_or(Error::AgentNotStarted)?
            };

            let mut hist = session.history.lock().map_err(|e| Error::BackendError {
                message: format!("Poisoned history lock: {e}"),
            })?;
            hist.clear();
            session.turn_count.store(0, Ordering::SeqCst);
            Ok(())
        }
    }

    fn last_response(
        &self,
        agent_id: AgentId,
    ) -> impl Future<Output = Result<Option<String>, Error>> + Send {
        let sessions_map = Arc::clone(&self.sessions);
        async move {
            let session = {
                let sessions = sessions_map.read().map_err(|e| Error::BackendError {
                    message: format!("Poisoned NATIVE_SESSIONS lock: {e}"),
                })?;
                sessions
                    .get(&agent_id)
                    .cloned()
                    .ok_or(Error::AgentNotStarted)?
            };

            let resp = session
                .last_response_text
                .lock()
                .map_err(|e| Error::BackendError {
                    message: format!("Poisoned last_response_text lock: {e}"),
                })?;
            Ok(resp.clone())
        }
    }

    fn compaction_indices(
        &self,
        agent_id: AgentId,
    ) -> impl Future<Output = Result<Vec<u32>, Error>> + Send {
        let sessions_map = Arc::clone(&self.sessions);
        async move {
            let session = {
                let sessions = sessions_map.read().map_err(|e| Error::BackendError {
                    message: format!("Poisoned NATIVE_SESSIONS lock: {e}"),
                })?;
                sessions
                    .get(&agent_id)
                    .cloned()
                    .ok_or(Error::AgentNotStarted)?
            };

            let indices = session
                .compaction_indices
                .lock()
                .map_err(|e| Error::BackendError {
                    message: format!("Poisoned compaction_indices lock: {e}"),
                })?;
            Ok(indices.clone())
        }
    }

    fn is_idle(&self, agent_id: AgentId) -> impl Future<Output = Result<bool, Error>> + Send {
        let sessions_map = Arc::clone(&self.sessions);
        async move {
            let session = {
                let sessions = sessions_map.read().map_err(|e| Error::BackendError {
                    message: format!("Poisoned NATIVE_SESSIONS lock: {e}"),
                })?;
                sessions
                    .get(&agent_id)
                    .cloned()
                    .ok_or(Error::AgentNotStarted)?
            };

            Ok(session.is_idle.load(Ordering::SeqCst))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{Audio, Content, Document, Image, Video};

    #[tokio::test]
    async fn test_native_runtime_creation_and_empty_counts() {
        let runtime = NativeRuntime::default();
        assert_eq!(runtime.active_agent_count().await.unwrap(), 0);
        assert!(runtime.shutdown_agent(999).await.is_ok());
    }

    #[test]
    fn test_to_usage_metadata() {
        let proto_usage = proto::localharness::UsageMetadata {
            prompt_token_count: 100,
            candidates_token_count: 50,
            total_token_count: 150,
            cached_content_token_count: 20,
            thoughts_token_count: 10,
            ..Default::default()
        };
        let usage = to_usage_metadata(&proto_usage);
        assert_eq!(usage.prompt_token_count, Some(100));
        assert_eq!(usage.candidates_token_count, Some(50));
        assert_eq!(usage.total_token_count, Some(150));
        assert_eq!(usage.cached_content_token_count, Some(20));
        assert_eq!(usage.thoughts_token_count, Some(10));
    }

    #[test]
    fn test_to_proto_user_input_multimodal() {
        use crate::content::ContentPrimitive;
        let content = Content::Multi {
            parts: vec![
                ContentPrimitive::Text {
                    text: "Explain this content".to_string(),
                },
                ContentPrimitive::Image(Image::png(vec![1, 2, 3])),
                ContentPrimitive::Audio(Audio::mp3(vec![4, 5, 6])),
                ContentPrimitive::Document(Document::pdf(vec![7, 8, 9])),
                ContentPrimitive::Video(Video::mp4(vec![10, 11, 12])),
            ],
        };

        let proto_input = to_proto_user_input(&content);
        assert_eq!(proto_input.parts.len(), 5);
    }
}

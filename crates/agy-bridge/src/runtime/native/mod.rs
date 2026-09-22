//! Native local harness runtime: communicates with the `localharness` binary
//! via stdio protobuf handshake and WebSocket JSON protocol without Python.

pub(crate) mod config;
pub(crate) mod events;
pub(crate) mod process;
pub(crate) mod session;
pub(crate) mod tools;
pub(crate) mod ws;

use std::{
    collections::{HashMap, HashSet},
    future::Future,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock, atomic::Ordering},
    time::Duration,
};

use tokio::sync::mpsc;

use self::{
    events::to_proto_user_input,
    process::HarnessProcess,
    session::NativeAgentSession,
    tools::build_available_tools,
    ws::{HarnessWsStream, connect_to_harness, initialize_harness, spawn_session_io_tasks},
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
    streaming::{ChatResponseHandle, ChatResponseWriter, channel_with_buffer},
    tools::AvailableTool,
    types::{ConversationMessage, MessageRole, UsageMetadata},
};

const WS_CONNECT_RETRY_COUNT: usize = 10;
const WS_CONNECT_RETRY_DELAY: Duration = Duration::from_millis(50);
const EVENT_CHANNEL_BUFFER_SIZE: usize = 64;

/// Running local harness instance with reference counting of active agents.
struct SharedHarnessEntry {
    process: HarnessProcess,
    active_agents: HashSet<AgentId>,
}

/// Per-`save_dir` harness slot protected by its own async mutex so concurrent
/// agent creations for different directories never block each other.
type HarnessSlot = Arc<tokio::sync::Mutex<Option<SharedHarnessEntry>>>;

fn native_bg_runtime() -> &'static tokio::runtime::Runtime {
    static NATIVE_BG_RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> =
        std::sync::OnceLock::new();
    NATIVE_BG_RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("agy-bridge-native-runtime")
            .build()
            .expect("Failed to initialize shared NativeRuntime Tokio runtime")
    })
}

/// Native local harness runtime communicating natively via WebSocket and Protobuf.
pub struct NativeRuntime {
    sessions: Arc<RwLock<HashMap<AgentId, Arc<NativeAgentSession>>>>,
    harnesses: Arc<Mutex<HashMap<PathBuf, HarnessSlot>>>,
    runtime_handle: tokio::runtime::Handle,
    config: RuntimeConfig,
}

impl NativeRuntime {
    /// Create a new native runtime instance.
    ///
    /// # Panics
    ///
    /// Panics if the shared multi-threaded Tokio runtime cannot be initialized.
    #[must_use]
    pub fn new(config: RuntimeConfig) -> Self {
        let runtime_handle = native_bg_runtime().handle().clone();
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            harnesses: Arc::new(Mutex::new(HashMap::new())),
            runtime_handle,
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
    // NOLINT: uniform async interface across Python and native runtime backends.
    #[allow(unknown_lints, clippy::unused_async, clippy::unused_async_trait_impl)]
    pub async fn active_agent_count(&self) -> Result<usize, Error> {
        let sessions = self.sessions.read().map_err(|e| Error::BackendError {
            message: format!("Poisoned NATIVE_SESSIONS lock: {e}"),
        })?;
        Ok(sessions.len())
    }

    /// Return the number of active `localharness` child processes managed by this runtime.
    pub async fn active_harness_count(&self) -> usize {
        let slots: Vec<HarnessSlot> = self
            .harnesses
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .cloned()
            .collect();
        let mut count = 0;
        for slot in slots {
            let mut guard = slot.lock().await;
            if let Some(ref mut entry) = *guard {
                if entry.process.is_alive() {
                    count += 1;
                } else {
                    *guard = None;
                }
            }
        }
        count
    }

    fn canonical_save_dir(save_dir: &Path) -> PathBuf {
        crate::policies::path::canonicalize_path(save_dir).unwrap_or_else(|e| {
            tracing::trace!(
                error = %e,
                path = %save_dir.display(),
                "Failed to canonicalize save_dir"
            );
            save_dir.to_path_buf()
        })
    }

    async fn deregister_agent_from_harnesses_map(
        harnesses_map: &Mutex<HashMap<PathBuf, HarnessSlot>>,
        save_dir: &Path,
        agent_id: AgentId,
    ) {
        let key = Self::canonical_save_dir(save_dir);
        let slot_opt = {
            match harnesses_map.lock() {
                Ok(map) => map.get(&key).cloned(),
                Err(e) => {
                    tracing::error!(
                        error = %e,
                        "Poisoned harnesses mutex in deregister_agent_from_harnesses_map"
                    );
                    None
                }
            }
        };
        if let Some(slot) = slot_opt {
            let mut guard = slot.lock().await;
            let should_kill = if let Some(ref mut entry) = *guard {
                entry.active_agents.remove(&agent_id);
                tracing::debug!(
                    agent_id = %agent_id,
                    remaining = entry.active_agents.len(),
                    "Deregistered agent from shared localharness"
                );
                entry.active_agents.is_empty()
            } else {
                false
            };
            if should_kill && let Some(mut entry) = guard.take() {
                tracing::info!(
                    agent_id = %agent_id,
                    save_dir = %key.display(),
                    "Last active agent shut down; terminating shared localharness process"
                );
                entry.process.kill().await;
            }
        }
    }

    fn get_or_create_harness_slot(&self, key: &Path) -> Result<HarnessSlot, Error> {
        let mut map = self.harnesses.lock().map_err(|e| Error::BackendError {
            message: format!("Poisoned harnesses mutex: {e}"),
        })?;
        Ok(map
            .entry(key.to_path_buf())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(None)))
            .clone())
    }

    async fn get_or_spawn_harness(
        &self,
        save_dir: &Path,
        custom_binary_path: Option<&Path>,
        agent_id: AgentId,
        force_new: bool,
    ) -> Result<(u16, String, bool), Error> {
        let key = Self::canonical_save_dir(save_dir);
        let slot = self.get_or_create_harness_slot(&key)?;
        let mut slot_guard = slot.lock().await;

        if !force_new
            && let Some(entry) = slot_guard.as_mut()
            && entry.process.is_alive()
        {
            entry.active_agents.insert(agent_id);
            tracing::debug!(
                agent_id = %agent_id,
                port = entry.process.port,
                active_agents = entry.active_agents.len(),
                "Reusing existing running localharness process for save_dir={}",
                key.display()
            );
            return Ok((entry.process.port, entry.process.api_key.clone(), true));
        }

        if let Some(mut old_entry) = slot_guard.take() {
            tracing::warn!(
                "Terminating stale localharness process for save_dir={} before respawning",
                key.display()
            );
            old_entry.process.start_kill();
        }

        // Spawn child process on NativeRuntime's dedicated long-lived Tokio runtime
        // so that dropping a caller's temporary Tokio runtime never kills localharness.
        let save_dir_owned = key.clone();
        let custom_owned = custom_binary_path.map(Path::to_path_buf);
        let rt_handle = self.runtime_handle.clone();
        let process = rt_handle
            .spawn(async move {
                HarnessProcess::spawn(&save_dir_owned, None, custom_owned.as_deref()).await
            })
            .await
            .map_err(|e| Error::BackendError {
                message: format!("Harness spawn task failed: {e}"),
            })??;

        let port = process.port;
        let api_key = process.api_key.clone();
        let mut active_agents = HashSet::new();
        active_agents.insert(agent_id);
        let entry = SharedHarnessEntry {
            process,
            active_agents,
        };
        *slot_guard = Some(entry);
        Ok((port, api_key, false))
    }

    async fn connect_and_init_harness(
        &self,
        save_dir: &Path,
        custom_binary_path: Option<&Path>,
        agent_id: AgentId,
        config: &AgentConfig,
        hook_runner: Option<&Arc<Hooks>>,
        policies: &PolicySet,
    ) -> Result<
        (
            HarnessWsStream,
            Option<String>,
            UsageMetadata,
            Option<crate::types::SandboxStatus>,
        ),
        Error,
    > {
        let (mut port, mut api_key, was_reused) = self
            .get_or_spawn_harness(save_dir, custom_binary_path, agent_id, false)
            .await?;

        let mut ws = match connect_to_harness(port, &api_key).await {
            Ok(ws) => ws,
            Err(_e) if was_reused => {
                tracing::warn!(
                    port,
                    "Failed to connect to reused localharness process; respawning a fresh instance"
                );
                let (new_port, new_key, _) = self
                    .get_or_spawn_harness(save_dir, custom_binary_path, agent_id, true)
                    .await?;
                port = new_port;
                api_key = new_key;
                connect_to_harness(port, &api_key).await?
            }
            Err(e) => return Err(e),
        };

        let (initial_cascade_id, initial_usage, initial_sandbox_status) =
            initialize_harness(&mut ws, config, hook_runner, policies).await?;
        Ok((
            ws,
            initial_cascade_id,
            initial_usage,
            initial_sandbox_status,
        ))
    }

    /// Terminate all managed localharness processes and wait for exit.
    ///
    /// # Errors
    ///
    /// Returns an error if process termination fails.
    pub async fn shutdown(&self) -> Result<(), Error> {
        let slots: Vec<HarnessSlot> = {
            let mut map = self.harnesses.lock().map_err(|e| Error::BackendError {
                message: format!("Poisoned harnesses mutex in shutdown: {e}"),
            })?;
            map.drain().map(|(_, s)| s).collect()
        };
        for slot in slots {
            let mut guard = slot.lock().await;
            if let Some(mut entry) = guard.take() {
                entry.process.kill().await;
            }
        }
        Ok(())
    }
}

impl Drop for NativeRuntime {
    fn drop(&mut self) {
        let mut map = match self.harnesses.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        for (_path, slot) in map.drain() {
            match slot.try_lock() {
                Ok(mut guard) => {
                    if let Some(mut entry) = guard.take() {
                        entry.process.start_kill();
                        self.runtime_handle.spawn(async move {
                            entry.process.kill().await;
                        });
                    }
                }
                Err(e) => {
                    tracing::debug!(error = %e, "NativeRuntime::drop: harness slot lock contended");
                }
            }
        }
    }
}

impl Default for NativeRuntime {
    fn default() -> Self {
        Self::new(RuntimeConfig::default())
    }
}

async fn acquire_session_turn(
    agent_id: AgentId,
    session: &NativeAgentSession,
) -> Result<(), Error> {
    if !session.connected.load(Ordering::SeqCst) {
        return Err(Error::BackendError {
            message: "Agent session is disconnected: the harness                       WebSocket has closed and cannot accept new turns"
                .to_string(),
        });
    }

    match session
        .is_idle
        .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
    {
        Ok(_) => Ok(()),
        Err(current_idle) => {
            tracing::debug!(
                agent_id = %agent_id,
                current_idle,
                "Agent session is not idle; checking if previous turn was abandoned"
            );
            let is_prev_abandoned = {
                let active = session.active_writer.lock().await;
                active
                    .as_ref()
                    .is_some_and(ChatResponseWriter::is_abandoned)
            };
            if is_prev_abandoned {
                tracing::debug!(
                    agent_id = %agent_id,
                    "Previous response handle was abandoned; halting previous turn and waiting for idle"
                );
                if let Err(e) = session
                    .event_tx
                    .send(proto::localharness::InputEvent {
                        event: Some(proto::localharness::input_event::Event::HaltRequest(true)),
                    })
                    .await
                {
                    tracing::debug!(
                        agent_id = %agent_id,
                        error = %e,
                        "Failed to send HaltRequest for abandoned turn"
                    );
                }
                while let Err(still_idle_state) = session.is_idle.compare_exchange(
                    true,
                    false,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                ) {
                    tracing::trace!(
                        agent_id = %agent_id,
                        still_idle_state,
                        "Waiting for abandoned turn to transition to idle"
                    );
                    if !session.connected.load(Ordering::SeqCst) {
                        return Err(Error::BackendError {
                            message: "Agent session disconnected while waiting for abandoned turn"
                                .to_string(),
                        });
                    }
                    session.idle_notify.notified().await;
                }
                Ok(())
            } else {
                tracing::debug!(
                    agent_id = %agent_id,
                    "Agent is busy with an active turn, cannot acquire turn"
                );
                Err(Error::BackendError {
                    message: "Agent is already executing a turn; wait for completion before starting a new turn".to_string(),
                })
            }
        }
    }
}

fn validate_create_config(config: &AgentConfig) -> Result<(), Error> {
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
    Ok(())
}

fn extract_initializing_hook_runner(agent_id: u64) -> Option<Arc<Hooks>> {
    let init_hooks = crate::runtime::initializing_hook_runners();
    match init_hooks.read() {
        Ok(guard) => guard.get(&agent_id).cloned(),
        Err(e) => {
            tracing::error!(error = %e, "Poisoned initializing_hook_runners lock");
            None
        }
    }
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
        let runtime = self.runtime_handle.clone();

        async move {
            validate_create_config(&config)?;
            let hook_runner = extract_initializing_hook_runner(agent_id);
            let policies = PolicySet::validated_from(config.policies.clone())?;

            let save_dir = config.save_dir.clone().unwrap_or_else(std::env::temp_dir);
            let (ws, initial_cascade_id, initial_usage, initial_sandbox_status) = match self
                .connect_and_init_harness(
                    &save_dir,
                    custom_binary_path.as_deref(),
                    agent_id,
                    &config,
                    hook_runner.as_ref(),
                    &policies,
                )
                .await
            {
                Ok(val) => val,
                Err(e) => {
                    Self::deregister_agent_from_harnesses_map(&self.harnesses, &save_dir, agent_id)
                        .await;
                    return Err(e);
                }
            };

            if let Some(ref id) = initial_cascade_id
                && let Err(e) =
                    crate::runtime::bridge_state::set_agent_conversation_id(agent_id, id.clone())
            {
                tracing::warn!(error = %e, "Failed to set agent conversation id on bridge_state");
            }

            let available_tools = build_available_tools(&config);
            let (event_tx, event_rx) =
                mpsc::channel::<proto::localharness::InputEvent>(EVENT_CHANNEL_BUFFER_SIZE);

            let session = Arc::new(NativeAgentSession::new(
                event_tx,
                initial_usage,
                initial_sandbox_status,
                save_dir,
            ));

            spawn_session_io_tasks(&runtime, agent_id, ws, &session, event_rx);

            {
                let mut sessions = sessions_map.write().map_err(|e| Error::BackendError {
                    message: format!("Poisoned NATIVE_SESSIONS lock: {e}"),
                })?;
                sessions.insert(agent_id, session);
            }

            if let Some(ref hooks) = hook_runner {
                let hooks = Arc::clone(hooks);
                let session_id = initial_cascade_id
                    .clone()
                    .unwrap_or_else(|| format!("agent-{agent_id}"));
                let ctx = OnSessionStartContext {
                    session: SessionContext {
                        session_id,
                        agent_id,
                        started_at: std::time::SystemTime::now(),
                    },
                };
                if let Err(e) =
                    tokio::task::spawn_blocking(move || hooks.run_on_session_start(&ctx)).await
                {
                    tracing::error!(error = %e, "on_session_start hook panicked in spawn_blocking");
                }
            }

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

            acquire_session_turn(agent_id, &session).await?;

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
                Content::Multi { parts } => parts
                    .iter()
                    .filter_map(|p| match p {
                        crate::content::ContentPrimitive::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
                _ => String::new(),
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
        let harnesses_map = Arc::clone(&self.harnesses);
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
                s.connected.store(false, Ordering::Release);

                Self::deregister_agent_from_harnesses_map(&harnesses_map, &s.save_dir, agent_id)
                    .await;

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
                    if let Err(e) =
                        tokio::task::spawn_blocking(move || hr.run_on_session_end(&ctx)).await
                    {
                        tracing::error!(error = %e, "on_session_end hook panicked in spawn_blocking");
                    }
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
                    s.connected.store(false, Ordering::Release);

                    let key = Self::canonical_save_dir(&s.save_dir);
                    let slot_opt = match self.harnesses.lock() {
                        Ok(harnesses) => harnesses.get(&key).cloned(),
                        Err(e) => {
                            tracing::debug!(
                                error = %e,
                                "try_shutdown: harnesses lock poisoned removing active agent"
                            );
                            None
                        }
                    };

                    if let Some(slot) = slot_opt {
                        match slot.try_lock() {
                            Ok(mut guard) => {
                                let should_kill = if let Some(ref mut entry) = *guard {
                                    entry.active_agents.remove(&agent_id);
                                    entry.active_agents.is_empty()
                                } else {
                                    false
                                };
                                if should_kill && let Some(mut entry) = guard.take() {
                                    entry.process.start_kill();
                                    self.runtime_handle.spawn(async move {
                                        entry.process.kill().await;
                                    });
                                }
                            }
                            Err(try_lock_err) => {
                                tracing::debug!(
                                    agent_id = %agent_id,
                                    error = %try_lock_err,
                                    "try_shutdown: harness slot lock contended, spawning async cleanup task"
                                );
                                let rt_handle = self.runtime_handle.clone();
                                let slot_clone = Arc::clone(&slot);
                                rt_handle.spawn(async move {
                                    let mut guard = slot_clone.lock().await;
                                    let should_kill = if let Some(ref mut entry) = *guard {
                                        entry.active_agents.remove(&agent_id);
                                        entry.active_agents.is_empty()
                                    } else {
                                        false
                                    };
                                    if should_kill && let Some(mut entry) = guard.take() {
                                        entry.process.kill().await;
                                    }
                                });
                            }
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

    fn sandbox_status(
        &self,
        agent_id: AgentId,
    ) -> impl Future<Output = Result<Option<crate::types::SandboxStatus>, Error>> + Send {
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

            Ok(session.sandbox_status.clone())
        }
    }
}

#[cfg(test)]
mod mod_tests;

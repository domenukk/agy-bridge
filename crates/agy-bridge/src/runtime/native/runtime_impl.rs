//! [`Runtime`] trait implementation for [`NativeRuntime`].

use std::{
    collections::{HashMap, HashSet},
    future::Future,
    sync::{Arc, RwLock, atomic::Ordering},
    time::Duration,
};

use tokio::sync::mpsc;

use super::{
    EVENT_CHANNEL_BUFFER_SIZE, NativeRuntime,
    events::to_proto_user_input,
    session::NativeAgentSession,
    tools::build_available_tools,
    ws::{remove_persisted_conversation_files, spawn_session_io_tasks},
};
use crate::{
    agent::{AgentId, Runtime, validate_create_config},
    config::AgentConfig,
    content::Content,
    error::Error,
    hooks::{Hooks, OnSessionEndContext, OnSessionStartContext, SessionContext},
    policies::PolicySet,
    proto,
    runtime::bridge_state::{agent_hooks, bridge_state},
    streaming::{ChatResponseHandle, ChatResponseWriter, channel_with_buffer},
    tools::AvailableTool,
    types::{ConversationMessage, MessageRole, UsageMetadata},
};

fn lookup_session(
    sessions_map: &RwLock<HashMap<AgentId, Arc<NativeAgentSession>>>,
    agent_id: AgentId,
) -> Result<Arc<NativeAgentSession>, Error> {
    let sessions = sessions_map.read().map_err(|e| Error::BackendError {
        message: format!("Poisoned NATIVE_SESSIONS lock: {e}"),
    })?;
    sessions
        .get(&agent_id)
        .cloned()
        .ok_or(Error::AgentNotStarted)
}

async fn acquire_session_turn(
    agent_id: AgentId,
    session: &NativeAgentSession,
) -> Result<(), Error> {
    if !session.connected.load(Ordering::SeqCst) {
        return Err(Error::BackendError {
            message: "Agent session is disconnected: the harness WebSocket has closed and cannot accept new turns"
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
            let hook_runner = agent_hooks(agent_id);
            let policies = PolicySet::validated_from(config.policies.clone())?;

            let save_dir = config.save_dir.clone().unwrap_or_else(std::env::temp_dir);
            if config.save_dir.is_some()
                && let Err(e) = std::fs::create_dir_all(&save_dir)
            {
                tracing::warn!(
                    path = %save_dir.display(),
                    error = %e,
                    "Failed to pre-create save_dir for localharness"
                );
            }
            let (ws, init_state) = match self
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

            let initial_cascade_id = init_state.cascade_id.clone();
            if let Some(ref id) = initial_cascade_id
                && let Err(e) =
                    crate::runtime::bridge_state::set_agent_conversation_id(agent_id, id.clone())
            {
                tracing::warn!(error = %e, "Failed to set agent conversation id on bridge_state");
            }

            let available_tools = build_available_tools(&config);
            let finish_on_tools: HashSet<String> = config.finish_on_tools.iter().cloned().collect();
            let (event_tx, event_rx) =
                mpsc::channel::<proto::localharness::InputEvent>(EVENT_CHANNEL_BUFFER_SIZE);

            let session = Arc::new(NativeAgentSession::new(
                event_tx,
                init_state,
                save_dir,
                finish_on_tools,
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
            let session = lookup_session(&sessions_map, agent_id)?;

            acquire_session_turn(agent_id, &session).await?;

            session.produced_output.store(false, Ordering::SeqCst);
            session.turn_activity.store(false, Ordering::SeqCst);
            session.finished_by_tool.store(false, Ordering::SeqCst);
            match session.last_response_text.lock() {
                Ok(mut resp_lock) => {
                    *resp_lock = None;
                }
                Err(e) => {
                    tracing::error!(error = %e, "Poisoned last_response_text lock in chat");
                }
            }
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
            let session = lookup_session(&sessions_map, agent_id)?;

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
            let session = lookup_session(&sessions_map, agent_id)?;

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
            let session = lookup_session(&sessions_map, agent_id)?;

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
            let session = lookup_session(&sessions_map, agent_id)?;

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
            let session = lookup_session(&sessions_map, agent_id)?;

            let hist = session.history.lock().map_err(|e| Error::BackendError {
                message: format!("Poisoned history lock: {e}"),
            })?;
            Ok(hist.clone())
        }
    }

    fn turn_count(&self, agent_id: AgentId) -> impl Future<Output = Result<u32, Error>> + Send {
        let sessions_map = Arc::clone(&self.sessions);
        async move {
            let session = lookup_session(&sessions_map, agent_id)?;
            Ok(session.turn_count.load(Ordering::Relaxed))
        }
    }

    fn total_usage(
        &self,
        agent_id: AgentId,
    ) -> impl Future<Output = Result<UsageMetadata, Error>> + Send {
        let sessions_map = Arc::clone(&self.sessions);
        async move {
            let session = lookup_session(&sessions_map, agent_id)?;

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
            let session = lookup_session(&sessions_map, agent_id)?;

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
            let session = lookup_session(&sessions_map, agent_id)?;

            let mut hist = session.history.lock().map_err(|e| Error::BackendError {
                message: format!("Poisoned history lock: {e}"),
            })?;
            hist.clear();
            let mut indices =
                session
                    .compaction_indices
                    .lock()
                    .map_err(|e| Error::BackendError {
                        message: format!("Poisoned compaction_indices lock: {e}"),
                    })?;
            indices.clear();
            let mut last_resp =
                session
                    .last_response_text
                    .lock()
                    .map_err(|e| Error::BackendError {
                        message: format!("Poisoned last_response_text lock: {e}"),
                    })?;
            *last_resp = None;
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
            let session = lookup_session(&sessions_map, agent_id)?;

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
            let session = lookup_session(&sessions_map, agent_id)?;

            let indices = session
                .compaction_indices
                .lock()
                .map_err(|e| Error::BackendError {
                    message: format!("Poisoned compaction_indices lock: {e}"),
                })?;
            Ok(indices.clone())
        }
    }

    fn delete(&self, agent_id: AgentId) -> impl Future<Output = Result<(), Error>> + Send {
        let sessions_map = Arc::clone(&self.sessions);
        let shutdown_fut = self.shutdown_agent(agent_id);
        async move {
            let save_dir_opt = match sessions_map.read() {
                Ok(s) => s.get(&agent_id).map(|sess| sess.save_dir.clone()),
                Err(e) => {
                    tracing::error!(error = %e, "Poisoned NATIVE_SESSIONS lock in delete");
                    None
                }
            };
            let conv_id = crate::runtime::bridge_state::get_agent_conversation_id(agent_id);
            let res = shutdown_fut.await;
            if let (Some(save_dir), Some(cid)) = (save_dir_opt, conv_id) {
                remove_persisted_conversation_files(&save_dir, &cid);
            }
            res
        }
    }

    fn disconnect(&self, agent_id: AgentId) -> impl Future<Output = Result<(), Error>> + Send {
        self.shutdown_agent(agent_id)
    }

    fn is_idle(&self, agent_id: AgentId) -> impl Future<Output = Result<bool, Error>> + Send {
        let sessions_map = Arc::clone(&self.sessions);
        async move {
            let session = lookup_session(&sessions_map, agent_id)?;
            Ok(session.is_idle.load(Ordering::SeqCst))
        }
    }

    fn sandbox_status(
        &self,
        agent_id: AgentId,
    ) -> impl Future<Output = Result<Option<crate::types::SandboxStatus>, Error>> + Send {
        let sessions_map = Arc::clone(&self.sessions);
        async move {
            let session = lookup_session(&sessions_map, agent_id)?;
            Ok(session.sandbox_status.clone())
        }
    }
}

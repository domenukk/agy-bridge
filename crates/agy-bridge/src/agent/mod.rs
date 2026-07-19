//! Agent lifecycle management for the Antigravity SDK bridge.
//!
//! Provides [`AgentHandle`](crate::agent::AgentHandle) which wraps the lifecycle of a single SDK agent:
//! creation, chatting, conversation tracking, and shutdown with RAII warnings.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use crate::{
    config::AgentConfig,
    content::Content,
    error::Error,
    streaming::{ChatResponseHandle, ChatResponseSharedState},
    types::{ConversationMessage, UsageMetadata},
};

#[cfg(test)]
pub(crate) mod mock;

/// Unique identifier for an agent within the bridge.
pub type AgentId = u64;

/// Trait abstracting the Python runtime interface.
///
/// This allows unit tests to inject a mock runtime without requiring a live
/// Python interpreter. The real implementation will call through to `PyO3`.
// NOLINT: async_fn_in_trait is intentional — Runtime is not object-safe by design
#[expect(
    async_fn_in_trait,
    reason = "Runtime is not object-safe by design; callers always know the concrete type"
)]
pub trait Runtime: Send + Sync {
    /// Create an agent from the given config, returning its ID and the list
    /// of all available tools (custom, MCP, and builtin) with metadata.
    ///
    /// `agent_id` is a process-globally-unique identifier allocated by the
    /// caller *before* creation, so per-agent initialization state can be
    /// registered under it without any cross-agent locking.
    async fn create_agent(
        &self,
        agent_id: u64,
        config: AgentConfig,
    ) -> Result<(AgentId, Vec<crate::tools::AvailableTool>), Error>;

    /// Send a chat message to the agent, returning a streaming response handle.
    ///
    /// The `content` parameter accepts any [`Content`] variant: plain text,
    /// images, documents, audio, video, or a multi-part list.
    async fn chat(&self, agent_id: AgentId, content: &Content)
    -> Result<ChatResponseHandle, Error>;

    /// Gracefully shut down the agent.
    async fn shutdown_agent(&self, agent_id: AgentId) -> Result<(), Error>;

    /// Interrupt any active prompt/chat run.
    async fn cancel(&self, agent_id: AgentId) -> Result<(), Error>;

    /// Wait for the active run or conversational loop to stabilize.
    async fn wait_for_idle(&self, agent_id: AgentId) -> Result<(), Error>;

    /// Send a message without waiting for completion.
    async fn send(&self, agent_id: AgentId, content: &Content) -> Result<(), Error>;

    /// Signal that the agent is idle.
    async fn signal_idle(&self, agent_id: AgentId) -> Result<(), Error>;

    /// Wait for the agent to wake up. Returns true if woken, false if timed out.
    async fn wait_for_wakeup(
        &self,
        agent_id: AgentId,
        timeout: std::time::Duration,
    ) -> Result<bool, Error>;

    /// Retrieve the conversation's message history.
    async fn history(&self, agent_id: AgentId) -> Result<Vec<ConversationMessage>, Error>;

    /// Return the number of completed turns in the conversation.
    async fn turn_count(&self, agent_id: AgentId) -> Result<u32, Error>;

    /// Return cumulative token usage across all turns.
    async fn total_usage(&self, agent_id: AgentId) -> Result<UsageMetadata, Error>;

    /// Return token usage from the most recent turn only.
    async fn last_turn_usage(&self, agent_id: AgentId) -> Result<UsageMetadata, Error>;

    /// Clear the conversation history and reset state.
    async fn clear_history(&self, agent_id: AgentId) -> Result<(), Error>;

    /// Return the text of the last model response, if any.
    ///
    /// Default implementation returns `Ok(None)`.
    async fn last_response(&self, _agent_id: AgentId) -> Result<Option<String>, Error> {
        Ok(None)
    }

    /// Return the step indices at which compaction occurred.
    ///
    /// Default implementation returns an empty list.
    async fn compaction_indices(&self, _agent_id: AgentId) -> Result<Vec<u32>, Error> {
        Ok(Vec::new())
    }

    /// Delete the conversation and all associated state.
    ///
    /// Default implementation is a no-op that returns `Ok(())`.
    async fn delete(&self, _agent_id: AgentId) -> Result<(), Error> {
        Ok(())
    }

    /// Disconnect from the agent without deleting state.
    ///
    /// Default implementation is a no-op that returns `Ok(())`.
    async fn disconnect(&self, _agent_id: AgentId) -> Result<(), Error> {
        Ok(())
    }

    /// Check whether the agent is currently idle (not running a turn).
    ///
    /// Default implementation returns `Ok(true)`.
    async fn is_idle(&self, _agent_id: AgentId) -> Result<bool, Error> {
        Ok(true)
    }

    /// Best-effort synchronous shutdown signal, called from [`Drop`].
    ///
    /// Unlike [`shutdown_agent`](Self::shutdown_agent), this is sync and
    /// fire-and-forget — it cannot return errors. The default is a no-op;
    /// implementations backed by a command channel should `try_send` a
    /// shutdown command here.
    fn try_shutdown_agent(&self, _agent_id: AgentId) {}
}

/// Handle to a running agent.
///
/// Wraps the agent's lifecycle: creation, chat, and shutdown.
///
/// Call [`shutdown()`](Self::shutdown) for a clean, error-reported shutdown.
/// If the handle is dropped without calling `shutdown()`, a best-effort
/// background shutdown is spawned via [`tokio::spawn`] — the Python agent
/// will be cleaned up, but errors are only logged, not returned.
///
/// Most methods take `&self` — interior mutability is used where needed
/// so multiple concurrent operations can share a single handle.
///
/// # Mutex choice
///
/// This type uses [`std::sync::Mutex`] rather than [`tokio::sync::Mutex`]
/// because every lock acquisition is a brief, synchronous operation (pointer
/// swap or clone) that **never** spans an `.await` point. For these
/// microsecond critical sections, `std::sync::Mutex` is both simpler and
/// lower-overhead than the async alternative.
pub struct AgentHandle<R: Runtime + 'static> {
    id: AgentId,
    runtime: Arc<R>,
    config: AgentConfig,
    /// Kept alive for the agent's lifetime so the global `BRIDGE_STATE`
    /// entry isn't the only strong reference.
    _registry: Option<Arc<crate::tools::ToolRegistry>>,
    /// Kept alive to preserve a strong reference to the policy confirmation handler.
    policy_handler: Option<Arc<dyn crate::policies::AskUserHandler>>,
    conversation_id: Arc<Mutex<Option<String>>>,
    is_started: AtomicBool,
    is_shutdown: AtomicBool,
    /// All tools available to this agent — custom Rust tools, MCP tools, and
    /// SDK builtins — with metadata about source, description, and schema.
    available_tools: Vec<crate::tools::AvailableTool>,
    /// Shared state from the last completed chat response, used to surface
    /// `get_last_structured_output()` without round-tripping to Python.
    ///
    /// Wrapped in a `Mutex` so `chat()` can take `&self` instead of `&mut self`,
    /// enabling concurrent usage patterns. The lock is brief (pointer swap only).
    last_shared_state: Mutex<Option<Arc<Mutex<ChatResponseSharedState>>>>,
}

/// RAII guard that removes an agent's entry from the initializing hook-runner
/// registry when dropped, guaranteeing no stale entry survives — whether
/// [`AgentHandle::new`] succeeds, returns early on error, or panics.
struct InitializingHookGuard(u64);

impl Drop for InitializingHookGuard {
    fn drop(&mut self) {
        match crate::runtime::initializing_hook_runners().write() {
            Ok(mut map) => {
                map.remove(&self.0);
            }
            Err(e) => {
                tracing::error!(
                    agent_id = self.0,
                    error = %e,
                    "initializing hook runners lock poisoned during cleanup — \
                     stale hook runner may persist"
                );
            }
        }
    }
}

impl<R: Runtime> AgentHandle<R> {
    /// Create a new agent from the given runtime and configuration.
    ///
    /// This sends a `CreateAgent` command to the Python runtime, waits for
    /// quota availability, and returns the handle.
    ///
    /// # Errors
    ///
    /// Returns a [`Error`] if agent creation fails (e.g. invalid config,
    /// Python error, or quota exceeded).
    pub async fn new(
        runtime: Arc<R>,
        config: AgentConfig,
        registry: Option<Arc<crate::tools::ToolRegistry>>,
        hook_runner: Option<Arc<crate::hooks::Hooks>>,
        policy_handler: Option<Arc<dyn crate::policies::AskUserHandler>>,
    ) -> Result<Self, Error> {
        // Allocate the (process-globally-unique) agent ID up front so we can
        // register per-agent initialization state *before* creation, keyed by
        // the ID. This avoids any process-wide lock across `create_agent`, so
        // concurrent creations — on the same or different bridges — never
        // block one another.
        let agent_id_u64 = crate::runtime::next_agent_id();

        let effective_hook_runner =
            hook_runner.unwrap_or_else(|| Arc::new(crate::hooks::Hooks::new()));

        // Install the hook runner in the per-agent initializing registry so
        // hooks that fire during `__aenter__` (before the permanent bridge
        // state exists) resolve correctly. `InitializingHookGuard` removes the
        // entry on every exit path, including early errors.
        match crate::runtime::initializing_hook_runners().write() {
            Ok(mut map) => {
                map.insert(agent_id_u64, Arc::clone(&effective_hook_runner));
            }
            Err(e) => {
                return Err(Error::BackendError {
                    message: format!(
                        "initializing hook runners lock poisoned — hooks cannot be installed: {e}"
                    ),
                });
            }
        }
        let _init_guard = InitializingHookGuard(agent_id_u64);

        let (agent_id, available_tools) =
            runtime.create_agent(agent_id_u64, config.clone()).await?;
        debug_assert_eq!(
            agent_id, agent_id_u64,
            "runtime must echo the caller-provided agent ID"
        );
        tracing::info!(agent_id, "Agent created successfully");

        let conversation_id = Self::setup_bridge_state(
            &runtime,
            agent_id,
            &config,
            registry.as_ref(),
            effective_hook_runner,
            policy_handler.as_ref(),
        )
        .await?;

        Ok(Self {
            id: agent_id,
            runtime,
            config,
            _registry: registry,
            policy_handler,
            conversation_id,
            is_started: AtomicBool::new(true),
            is_shutdown: AtomicBool::new(false),
            available_tools,
            last_shared_state: Mutex::new(None),
        })
    }

    async fn setup_bridge_state(
        runtime: &Arc<R>,
        id: AgentId,
        config: &AgentConfig,
        registry: Option<&Arc<crate::tools::ToolRegistry>>,
        effective_hook_runner: Arc<crate::hooks::Hooks>,
        policy_handler: Option<&Arc<dyn crate::policies::AskUserHandler>>,
    ) -> Result<Arc<Mutex<Option<String>>>, Error> {
        let policies_set = crate::policies::PolicySet::validated_from(config.policies.clone())?;
        let conversation_id = Arc::new(Mutex::new(config.conversation_id.clone()));
        let bridge_entry = crate::runtime::AgentBridgeState {
            registry: registry.map(Arc::clone),
            hook_runner: Some(effective_hook_runner),
            policies: policies_set,
            policy_handler: policy_handler.map(Arc::clone),
            tool_state: llm_tool::SharedState::new(),
            conversation_id: Arc::clone(&conversation_id),
            last_tool_error: std::sync::Mutex::new(None),
        };
        let bridge_insert_failed = match crate::runtime::bridge_state().write() {
            Ok(mut map) => {
                map.insert(id, bridge_entry);
                false
            }
            Err(e) => {
                tracing::error!(
                    agent_id = id,
                    error = %e,
                    "Failed to acquire write lock on BRIDGE_STATE — agent would be unusable"
                );
                true
            }
        };
        if bridge_insert_failed {
            if let Err(shutdown_err) = runtime.shutdown_agent(id).await {
                tracing::error!(
                    agent_id = id,
                    error = ?shutdown_err,
                    "Failed to shut down agent after BRIDGE_STATE lock failure"
                );
            }
            return Err(Error::BackendError {
                message: "BRIDGE_STATE RwLock poisoned during agent creation".to_string(),
            });
        }
        Ok(conversation_id)
    }

    /// Send a message and receive a streaming response.
    ///
    /// Accepts any type that converts into [`Content`]: `&str`, `String`,
    /// [`Image`](crate::content::Image), [`Document`](crate::content::Document),
    /// [`Audio`](crate::content::Audio), [`Video`](crate::content::Video), or a
    /// `Vec<ContentPrimitive>` for multimodal input.
    ///
    /// This is **single-shot**: a quota error (HTTP 429) or any other failure
    /// is returned immediately. Retrying is the caller's responsibility, so a
    /// caller running its own retry loop is never double-retried.
    ///
    /// # Errors
    ///
    /// Returns a [`Error`] on chat failure (Python error, timeout, etc.).
    pub async fn chat(&self, content: impl Into<Content>) -> Result<ChatResponseHandle, Error> {
        if !self.is_started() {
            return Err(Error::AgentNotStarted);
        }
        self.chat_once(&content.into()).await
    }

    /// Perform a single (non-retrying) chat turn, recording the streaming
    /// shared state for later `get_last_structured_output()` access.
    async fn chat_once(&self, content: &Content) -> Result<ChatResponseHandle, Error> {
        let handle = self.runtime.chat(self.id, content).await?;
        match self.last_shared_state.lock() {
            Ok(mut guard) => {
                *guard = Some(Arc::clone(&handle.shared_state));
            }
            Err(e) => {
                tracing::error!(
                    agent_id = self.id,
                    error = %e,
                    "last_shared_state mutex poisoned — streaming metadata may be stale"
                );
            }
        }
        Ok(handle)
    }

    /// Send a message and return the final text response.
    ///
    /// This is a convenience wrapper around [`chat`](Self::chat) that drains
    /// the streaming response into a single `String`. If tools were associated
    /// with the agent at creation time, the Python runtime handles tool
    /// execution automatically.
    ///
    /// Like [`chat`](Self::chat), this is **single-shot**: quota / 429 errors
    /// that surface *during* streaming are returned to the caller immediately.
    /// Retrying is the caller's responsibility.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the chat turn fails or stream errors occur.
    pub async fn chat_text(&self, message: impl Into<Content>) -> Result<String, Error> {
        self.chat_text_once(&message.into()).await
    }

    /// Perform a single (non-retrying) chat turn and drain it to text.
    ///
    /// A stream error encountered while draining is mapped to a
    /// [`Error::BackendError`], preserving [`Error::is_quota_error`] semantics so
    /// the caller's retry loop can react to quota errors that only surface here.
    async fn chat_text_once(&self, content: &Content) -> Result<String, Error> {
        let response = self.chat_once(content).await?;
        match response.text().await {
            Ok(text) => Ok(text.into_string()),
            Err(stream_err) => Err(Error::BackendError {
                message: format!(
                    "Failed to read response text: stream error: {}",
                    stream_err.message
                ),
            }),
        }
    }

    /// Return the current conversation ID, if one has been set.
    ///
    /// Returns a cloned `String` because the underlying value is behind a
    /// [`Mutex`] (interior mutability for `&self` access).
    #[must_use]
    pub fn conversation_id(&self) -> Option<String> {
        self.conversation_id
            .lock()
            .inspect_err(|e| {
                tracing::error!(
                    agent_id = self.id,
                    error = %e,
                    "conversation_id mutex poisoned"
                );
            })
            // NOLINT: error already logged via inspect_err above; .ok() converts to Option for the return type
            .ok()
            .and_then(|guard| guard.clone())
    }

    /// Set the conversation ID (called when the SDK assigns one).
    ///
    /// Takes `&self` rather than `&mut self` so the handle can be shared
    /// across concurrent tasks.
    pub fn set_conversation_id(&self, id: String) {
        match self.conversation_id.lock() {
            Ok(mut guard) => {
                *guard = Some(id);
            }
            Err(e) => {
                tracing::error!(
                    agent_id = self.id,
                    error = %e,
                    "conversation_id mutex poisoned — ID will not be updated"
                );
            }
        }
    }

    /// Check whether the agent has been started and is not yet shut down.
    #[must_use]
    pub fn is_started(&self) -> bool {
        self.is_started.load(Ordering::SeqCst) && !self.is_shutdown.load(Ordering::SeqCst)
    }

    /// Return the agent's unique identifier.
    #[must_use]
    pub const fn id(&self) -> AgentId {
        self.id
    }

    /// Return a reference to the agent's configuration.
    #[must_use]
    pub const fn config(&self) -> &AgentConfig {
        &self.config
    }

    /// Return all tools available to this agent, with metadata.
    ///
    /// Each [`AvailableTool`](crate::tools::AvailableTool) includes the tool's
    /// name, description, JSON parameter schema, and source tag
    /// ([`Builtin`](crate::tools::ToolSource::Builtin),
    /// [`Custom`](crate::tools::ToolSource::Custom), or
    /// [`Mcp`](crate::tools::ToolSource::Mcp)).
    ///
    /// The list is assembled at agent creation time and is immutable for
    /// the agent's lifetime.
    #[must_use]
    pub fn available_tools(&self) -> &[crate::tools::AvailableTool] {
        &self.available_tools
    }

    /// Convenience accessor: returns just the tool names.
    #[must_use]
    pub fn available_tool_names(&self) -> Vec<&str> {
        self.available_tools
            .iter()
            .map(|t| t.name.as_str())
            .collect()
    }

    /// Interrupt the active chat prompt execution.
    ///
    /// # Errors
    ///
    /// Returns a [`Error`] if the cancellation call fails.
    pub async fn cancel(&self) -> Result<(), Error> {
        self.runtime.cancel(self.id).await
    }

    /// Wait for the conversation or active run to stabilize and become idle.
    ///
    /// # Errors
    ///
    /// Returns a [`Error`] if the wait call fails.
    pub async fn wait_for_idle(&self) -> Result<(), Error> {
        self.runtime.wait_for_idle(self.id).await
    }

    /// Retrieve the conversation's message history.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the query fails.
    pub async fn history(&self) -> Result<Vec<ConversationMessage>, Error> {
        self.runtime.history(self.id).await
    }

    /// Return the number of completed turns in the conversation.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the query fails.
    pub async fn turn_count(&self) -> Result<u32, Error> {
        self.runtime.turn_count(self.id).await
    }

    /// Return cumulative token usage across all turns.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the query fails.
    pub async fn total_usage(&self) -> Result<UsageMetadata, Error> {
        self.runtime.total_usage(self.id).await
    }

    /// Return token usage from the most recent turn only.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the query fails.
    pub async fn last_turn_usage(&self) -> Result<UsageMetadata, Error> {
        self.runtime.last_turn_usage(self.id).await
    }

    /// Clear the conversation history and reset state.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the operation fails.
    pub async fn clear_history(&self) -> Result<(), Error> {
        self.runtime.clear_history(self.id).await
    }

    /// Return the text of the last model response, if any.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the query fails.
    pub async fn last_response(&self) -> Result<Option<String>, Error> {
        self.runtime.last_response(self.id).await
    }

    /// Return the step indices at which conversation compaction occurred.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the query fails.
    pub async fn compaction_indices(&self) -> Result<Vec<u32>, Error> {
        self.runtime.compaction_indices(self.id).await
    }

    /// Delete the conversation and all associated state.
    ///
    /// After calling this method, the agent handle is no longer usable
    /// for chat operations. This also marks the agent as shut down.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the delete operation fails.
    pub async fn delete(&self) -> Result<(), Error> {
        let result = self.runtime.delete(self.id).await;
        self.is_shutdown.store(true, Ordering::SeqCst);
        result
    }

    /// Disconnect from the agent without deleting its state.
    ///
    /// The agent's conversation state is preserved but this handle
    /// can no longer send messages. Marks the agent as shut down.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the disconnect operation fails.
    pub async fn disconnect(&self) -> Result<(), Error> {
        let result = self.runtime.disconnect(self.id).await;
        self.is_shutdown.store(true, Ordering::SeqCst);
        result
    }

    /// Check whether the agent is currently idle (not running a turn).
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the query fails.
    pub async fn is_idle(&self) -> Result<bool, Error> {
        self.runtime.is_idle(self.id).await
    }

    /// Return the structured output from the last chat response, if any.
    ///
    /// Only populated after a [`chat()`](Self::chat) round-trip when the
    /// agent was configured with a `response_schema` and the model returned
    /// a valid JSON payload.
    #[must_use]
    pub fn get_last_structured_output(&self) -> Option<serde_json::Value> {
        let guard = self
            .last_shared_state
            .lock()
            .inspect_err(|e| {
                tracing::error!(
                    agent_id = self.id,
                    error = %e,
                    "last_shared_state mutex poisoned in get_last_structured_output"
                );
            })
            // NOLINT: error already logged via inspect_err above; .ok()? propagates None on poison
            .ok()?;
        let state = guard
            .as_ref()?
            .lock()
            .inspect_err(|e| {
                tracing::error!(
                    agent_id = self.id,
                    error = %e,
                    "ChatResponseSharedState mutex poisoned in get_last_structured_output"
                );
            })
            // NOLINT: error already logged via inspect_err above; .ok()? propagates None on poison
            .ok()?;
        state.structured_output.clone()
    }

    /// Return the structured output from the last chat response deserialized into `T`.
    ///
    /// Returns `None` if there was no structured output on the last response.
    /// Returns `Some(Err(...))` if the structured output could not be deserialized as `T`.
    pub fn get_last_structured_output_as<T: serde::de::DeserializeOwned>(
        &self,
    ) -> Option<Result<T, serde_json::Error>> {
        self.get_last_structured_output()
            .map(serde_json::from_value)
    }

    /// Return the usage metadata from the last chat response, if any.
    #[must_use]
    pub fn get_last_usage(&self) -> Option<UsageMetadata> {
        let guard = self
            .last_shared_state
            .lock()
            .inspect_err(|e| {
                tracing::error!(
                    agent_id = self.id,
                    error = %e,
                    "last_shared_state mutex poisoned in get_last_usage"
                );
            })
            // NOLINT: error already logged via inspect_err above; .ok()? propagates None on poison
            .ok()?;
        let state = guard
            .as_ref()?
            .lock()
            .inspect_err(|e| {
                tracing::error!(
                    agent_id = self.id,
                    error = %e,
                    "ChatResponseSharedState mutex poisoned in get_last_usage"
                );
            })
            // NOLINT: error already logged via inspect_err above; .ok()? propagates None on poison
            .ok()?;
        state.usage.clone()
    }

    /// Send a message without waiting for a response.
    ///
    /// Fire-and-forget: the message is delivered to the agent but no
    /// streaming response is produced.
    ///
    /// This is **single-shot**; retrying is the caller's responsibility.
    ///
    /// # Errors
    ///
    /// Returns a [`Error`] if sending fails.
    pub async fn send(&self, content: impl Into<Content>) -> Result<(), Error> {
        if !self.is_started() {
            return Err(Error::AgentNotStarted);
        }
        self.runtime.send(self.id, &content.into()).await
    }

    /// Signal that this agent is idle and ready to receive input.
    ///
    /// # Errors
    ///
    /// Returns a [`Error`] if the signal call fails.
    pub async fn signal_idle(&self) -> Result<(), Error> {
        self.runtime.signal_idle(self.id).await
    }

    /// Wait for the agent to wake up, returning `true` if woken or
    /// `false` if the `timeout` elapsed.
    ///
    /// # Errors
    ///
    /// Returns a [`Error`] if the wait call fails.
    pub async fn wait_for_wakeup(&self, timeout: std::time::Duration) -> Result<bool, Error> {
        self.runtime.wait_for_wakeup(self.id, timeout).await
    }

    /// Gracefully shut down the agent.
    ///
    /// This sends a `ShutdownAgent` command to the Python runtime, which
    /// calls `__aexit__()` on the SDK agent. The handle remains usable
    /// for read-only queries (e.g. [`is_started()`](Self::is_started))
    /// after shutdown.
    ///
    /// # Errors
    ///
    /// Returns a [`Error`] if shutdown fails. The `is_shutdown`
    /// flag is always set so the `Drop` impl will not emit a warning.
    pub async fn shutdown(&self) -> Result<(), Error> {
        if self.is_shutdown.load(Ordering::SeqCst) {
            tracing::debug!(agent_id = self.id, "Agent already shut down");
            return Ok(());
        }

        tracing::info!(agent_id = self.id, "Shutting down agent");
        let result = self.runtime.shutdown_agent(self.id).await;

        // Always mark as shut down so Drop doesn't warn, even on failure.
        self.is_shutdown.store(true, Ordering::SeqCst);

        // Clean up bridge state AFTER the runtime's shutdown completes.
        // In the live runtime, `__aexit__` fires hooks (e.g. on_session_end)
        // that look up bridge state — so this must happen after, not before.
        match crate::runtime::bridge_state().write() {
            Ok(mut map) => {
                map.remove(&self.id);
            }
            Err(e) => {
                tracing::error!(
                    agent_id = self.id,
                    error = %e,
                    "BRIDGE_STATE RwLock poisoned during shutdown cleanup — \
                     bridge state entry may leak"
                );
            }
        }

        match result {
            Ok(()) => {
                tracing::info!(agent_id = self.id, "Agent shut down successfully");
            }
            Err(ref e) => {
                tracing::error!(agent_id = self.id, error = ?e, "Agent shutdown failed");
            }
        }

        result
    }

    /// Spawn a subagent from the given config, sharing this agent's runtime.
    ///
    /// If a `ToolRegistry` is provided and `config.tools` is empty, the
    /// registry's definitions are automatically applied.
    ///
    /// # Errors
    ///
    /// Returns a [`Error`] if agent creation fails.
    pub async fn spawn_subagent(
        &self,
        mut config: AgentConfig,
        registry: impl Into<Option<crate::tools::ToolRegistry>>,
    ) -> Result<Self, Error> {
        let opt_registry = registry.into();
        if let Some(disp) = &opt_registry
            && config.tools.is_empty()
        {
            config.tools = disp.definitions();
        }
        let arc_registry = opt_registry.map(Arc::new);
        Self::new(
            Arc::clone(&self.runtime),
            config,
            arc_registry,
            None,
            self.policy_handler.clone(),
        )
        .await
    }
}

impl<R: Runtime> Drop for AgentHandle<R> {
    fn drop(&mut self) {
        if self.is_started.load(Ordering::SeqCst) && !self.is_shutdown.load(Ordering::SeqCst) {
            tracing::debug!(
                agent_id = self.id,
                "AgentHandle dropped without explicit shutdown() — \
                 sending best-effort shutdown signal"
            );
            // try_shutdown_agent fires a command that eventually calls
            // handle_shutdown_agent, which cleans up bridge state AFTER
            // __aexit__ completes (so on_session_end hooks can still
            // find the hook runner). Do NOT clean up bridge state here.
            self.runtime.try_shutdown_agent(self.id);
        } else if self.is_shutdown.load(Ordering::SeqCst) {
            // shutdown() was already called — handle_shutdown_agent
            // already cleaned up bridge state after __aexit__. Nothing
            // to do.
        } else {
            // Agent was never started (e.g. creation failed). Clean up
            // any partial bridge state that might have been registered.
            match crate::runtime::bridge_state().write() {
                Ok(mut map) => {
                    map.remove(&self.id);
                }
                Err(e) => {
                    tracing::warn!(
                        agent_id = self.id,
                        error = %e,
                        "BRIDGE_STATE RwLock poisoned during Drop — \
                         bridge state entry for this agent may leak"
                    );
                }
            }
        }
    }
}

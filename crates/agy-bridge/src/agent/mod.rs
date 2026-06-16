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

/// Default backoff duration when a quota/429 error doesn't include a
/// `Retry-After` header.
const DEFAULT_QUOTA_BACKOFF: std::time::Duration = std::time::Duration::from_secs(2);

/// Duration reported to the caller when all quota retries are exhausted.
const QUOTA_EXHAUSTED_RETRY_AFTER: std::time::Duration = std::time::Duration::from_mins(2);

#[cfg(test)]
pub(crate) mod mock;

/// Unique identifier for an agent within the bridge.
pub type AgentId = u64;

/// Trait abstracting the Python runtime interface.
///
/// This allows unit tests to inject a mock runtime without requiring a live
/// Python interpreter. The real implementation will call through to `PyO3`.
#[expect(
    async_fn_in_trait,
    reason = "Runtime is not object-safe by design; callers always know the concrete type"
)]
pub trait Runtime: Send + Sync {
    /// Create an agent from the given config, returning its ID.
    async fn create_agent(&self, config: AgentConfig) -> Result<AgentId, Error>;

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

    /// Wait if we're in a quota backoff period.
    async fn wait_for_quota(&self);

    /// Record a quota hit with the suggested retry duration.
    async fn record_quota_hit(&self, retry_after: std::time::Duration);

    /// Access this runtime's per-key quota registry.
    ///
    /// Each runtime owns its own [`QuotaRegistry`](crate::quota::QuotaRegistry),
    /// so different runtimes have fully independent quota tracking.
    fn quota_registry(&self) -> &crate::quota::QuotaRegistry;

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
    /// Per-API-key quota state. Agents sharing the same effective API key
    /// share backoff tracking; agents with different keys are independent.
    quota_state: Arc<crate::quota::QuotaState>,
    /// Kept alive for the agent's lifetime so the global `BRIDGE_STATE`
    /// entry isn't the only strong reference.
    _registry: Option<Arc<crate::tools::ToolRegistry>>,
    /// Kept alive to preserve a strong reference to the policy confirmation handler.
    policy_handler: Option<Arc<dyn crate::policies::AskUserHandler>>,
    conversation_id: Mutex<Option<String>>,
    is_started: AtomicBool,
    is_shutdown: AtomicBool,
    /// Shared state from the last completed chat response, used to surface
    /// `get_last_structured_output()` without round-tripping to Python.
    ///
    /// Wrapped in a `Mutex` so `chat()` can take `&self` instead of `&mut self`,
    /// enabling concurrent usage patterns. The lock is brief (pointer swap only).
    last_shared_state: Mutex<Option<Arc<Mutex<ChatResponseSharedState>>>>,
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
        let quota_key = config.effective_api_key().unwrap_or_default();
        let quota_state = runtime.quota_registry().state_for_key(&quota_key);
        let id = if hook_runner.is_some() {
            // Serialize the set→create→clear sequence so concurrent creates
            // cannot overwrite each other's temporary hook runner.
            //
            // NOTE: These must remain process-global because the Python-side
            // callback (`dispatch_rust_hook`) is itself process-global — it is
            // registered in `sys.modules["_agy_bridge_globals"]` and has no way
            // to identify which runtime instance triggered it. A per-runtime
            // guard would not prevent cross-runtime races on the shared
            // INITIALIZING_HOOK_RUNNER slot.
            let _guard = crate::runtime::CREATE_AGENT_HOOK_GUARD.lock().await;
            if let Ok(mut opt) = crate::runtime::INITIALIZING_HOOK_RUNNER.lock() {
                *opt = hook_runner.as_ref().map(Arc::clone);
            } else {
                tracing::error!("INITIALIZING_HOOK_RUNNER mutex poisoned — hook may not fire");
            }
            let result = runtime.create_agent(config.clone()).await;
            if let Ok(mut opt) = crate::runtime::INITIALIZING_HOOK_RUNNER.lock() {
                *opt = None;
            } else {
                tracing::error!("INITIALIZING_HOOK_RUNNER mutex poisoned — stale hook may persist");
            }
            result?
        } else {
            runtime.create_agent(config.clone()).await?
        };

        tracing::info!(agent_id = id, "Agent created successfully");

        // Build and insert per-agent bridge state in a single lock acquisition.
        let policies_set = crate::policies::PolicySet::validated_from(config.policies.clone())?;
        let bridge_entry = crate::runtime::AgentBridgeState {
            registry: registry.as_ref().map(Arc::clone),
            hook_runner: hook_runner.as_ref().map(Arc::clone),
            policies: policies_set,
            policy_handler: policy_handler.as_ref().map(Arc::clone),
            tool_state: Arc::new(std::sync::RwLock::new(std::collections::HashMap::new())),
        };
        if let Ok(mut map) = crate::runtime::bridge_state().write() {
            map.insert(id, bridge_entry);
        } else {
            tracing::error!(
                agent_id = id,
                "Failed to acquire write lock on BRIDGE_STATE"
            );
        }

        let conversation_id = Mutex::new(config.conversation_id.clone());
        Ok(Self {
            id,
            runtime,
            config,
            quota_state,
            _registry: registry,
            policy_handler,
            conversation_id,
            is_started: AtomicBool::new(true),
            is_shutdown: AtomicBool::new(false),
            last_shared_state: Mutex::new(None),
        })
    }

    /// Send a message and receive a streaming response.
    ///
    /// Accepts any type that converts into [`Content`]: `&str`, `String`,
    /// [`Image`](crate::content::Image), [`Document`](crate::content::Document),
    /// [`Audio`](crate::content::Audio), [`Video`](crate::content::Video), or a
    /// `Vec<ContentPrimitive>` for multimodal input.
    ///
    /// Automatically backs off on quota limits (HTTP 429).
    ///
    /// # Errors
    ///
    /// Returns a [`Error`] on chat failure (Python error, timeout, etc.).
    pub async fn chat(&self, content: impl Into<Content>) -> Result<ChatResponseHandle, Error> {
        if !self.is_started() {
            return Err(Error::AgentNotStarted);
        }

        let content = content.into();
        let max_retries = self.config.max_quota_retries.unwrap_or(0);

        let handle = 'retry: {
            for attempt in 0..=max_retries {
                if attempt > 0 {
                    self.quota_state.wait_for_quota().await;
                }
                match self.runtime.chat(self.id, &content).await {
                    Ok(h) => break 'retry h,
                    Err(Error::QuotaExceeded { retry_after }) => {
                        self.handle_quota_error("chat", attempt, max_retries, retry_after)?;
                    }
                    Err(ref e) if e.is_quota_error() => {
                        self.handle_quota_error(
                            "chat",
                            attempt,
                            max_retries,
                            DEFAULT_QUOTA_BACKOFF,
                        )?;
                    }
                    Err(e) => return Err(e),
                }
            }
            return Err(Error::QuotaExceeded {
                retry_after: QUOTA_EXHAUSTED_RETRY_AFTER,
            });
        };

        if let Ok(mut guard) = self.last_shared_state.lock() {
            *guard = Some(Arc::clone(&handle.shared_state));
        } else {
            tracing::error!("last_shared_state mutex poisoned — streaming metadata may be stale");
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
    /// # Errors
    ///
    /// Returns [`Error`] if the chat turn fails or stream errors occur.
    pub async fn chat_text(&self, message: impl Into<Content>) -> Result<String, Error> {
        let response = self.chat(message.into()).await?;
        let text = response.text().await.map_err(|e| {
            let converted = Error::from(e);
            if matches!(converted, Error::Safety) {
                converted
            } else {
                Error::BackendError {
                    message: format!("Failed to read response text: {converted}"),
                }
            }
        })?;
        Ok(text.into_string())
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
            .ok()
            .and_then(|guard| guard.clone())
    }

    /// Set the conversation ID (called when the SDK assigns one).
    ///
    /// Takes `&self` rather than `&mut self` so the handle can be shared
    /// across concurrent tasks.
    pub fn set_conversation_id(&self, id: String) {
        if let Ok(mut guard) = self.conversation_id.lock() {
            *guard = Some(id);
        } else {
            tracing::error!("Failed to acquire lock on conversation_id");
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
            .ok()?;
        state.structured_output.clone()
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
            .ok()?;
        state.usage.clone()
    }

    /// Send a message without waiting for a response.
    ///
    /// Fire-and-forget: the message is delivered to the agent but no
    /// streaming response is produced.
    ///
    /// # Errors
    ///
    /// Returns a [`Error`] if sending fails.
    pub async fn send(&self, content: impl Into<Content>) -> Result<(), Error> {
        if !self.is_started() {
            return Err(Error::AgentNotStarted);
        }

        let content = content.into();

        let max_retries = self.config.max_quota_retries.unwrap_or(0);

        for attempt in 0..=max_retries {
            if attempt > 0 {
                self.quota_state.wait_for_quota().await;
            }
            match self.runtime.send(self.id, &content).await {
                Ok(()) => return Ok(()),
                Err(Error::QuotaExceeded { retry_after }) => {
                    self.handle_quota_error("send", attempt, max_retries, retry_after)?;
                }
                Err(ref e) if e.is_quota_error() => {
                    self.handle_quota_error("send", attempt, max_retries, DEFAULT_QUOTA_BACKOFF)?;
                }
                Err(e) => return Err(e),
            }
        }
        Err(Error::QuotaExceeded {
            retry_after: QUOTA_EXHAUSTED_RETRY_AFTER,
        })
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

    /// Handle a quota/429 error from a retryable operation.
    fn handle_quota_error(
        &self,
        operation: &str,
        attempt: u32,
        max_retries: u32,
        retry_after: std::time::Duration,
    ) -> Result<(), Error> {
        if attempt >= max_retries {
            return Err(Error::QuotaExceeded { retry_after });
        }
        tracing::warn!(
            agent_id = self.id,
            attempt = attempt + 1,
            max = max_retries,
            retry_after_ms = u64::try_from(retry_after.as_millis()).unwrap_or_else(|e| {
                tracing::warn!("Int conversion failed: {e}");
                u64::MAX
            }),
            "Quota exceeded on {operation} — recording hit and retrying"
        );
        self.quota_state.record_quota_hit(retry_after);
        Ok(())
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
            self.runtime.try_shutdown_agent(self.id);
        }

        // Clean up global bridge state entry.
        if let Ok(mut map) = crate::runtime::bridge_state().write() {
            map.remove(&self.id);
        } else {
            tracing::error!(
                agent_id = self.id,
                "BRIDGE_STATE RwLock poisoned during Drop — \
                 bridge state entry for this agent may leak"
            );
        }
    }
}

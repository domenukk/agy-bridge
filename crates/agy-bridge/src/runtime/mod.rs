//! Python runtime manager: owns a dedicated Python thread with an asyncio event loop.
//!
//! The `PythonRuntime` struct bridges Rust's tokio async world with Python's asyncio
//! by running a command dispatch loop in a dedicated thread. Rust sends `PyCommand`
//! messages via an `mpsc` channel, and receives results via per-command `oneshot` channels.
//!
//! # Threading architecture
//!
//! - **One Python thread**: All GIL acquisition is confined to a single dedicated thread
//!   (`agy-bridge-python-runtime`). This thread runs an asyncio event loop via
//!   `pyo3_async_runtimes::tokio::run_until_complete`.
//!
//! - **Concurrent command processing**: Commands received from the `mpsc` channel are
//!   **not** serialized. Each command spawns a future into a `FuturesUnordered` task set,
//!   and `tokio::select!` drives both incoming commands and in-flight task completions.
//!   Multiple chats/operations run concurrently through the Python asyncio event loop.
//!
//! - **Rust tool dispatch**: When the Python SDK invokes a Rust tool, `dispatch_rust_tool`
//!   reads tool state from `BRIDGE_STATE`, then uses `future_into_py` to run the async
//!   tool on the tokio runtime — keeping the Python thread unblocked for other coroutines.
//!
//! - **Hook/policy dispatch**: Similarly, `dispatch_rust_hook` and `dispatch_rust_policy_confirm`
//!   use `spawn_blocking` to run synchronous hook callbacks without holding the GIL.
//!
//! # Why global state (`BRIDGE_STATE`)?
//!
//! The Python SDK's tool/hook/policy callbacks are dispatched via PyO3 `#[pyfunction]`
//! entries (e.g. `dispatch_rust_tool`, `dispatch_rust_hook`). These functions are
//! registered as plain Python callables and receive **only** the arguments the SDK
//! passes (agent ID + serialized context). There is no way to thread a Rust reference
//! or `Arc` through the Python call boundary.
//!
//! Therefore per-agent state (tool registries, hook runners, policy sets) is stored in
//! a global `RwLock<HashMap<AgentId, AgentBridgeState>>`. The agent ID is used as a
//! lookup key, and the lock is held only for brief `HashMap` operations (never across
//! `.await` points). This is the standard pattern for PyO3 FFI bridges that need to
//! associate Rust state with Python-side identifiers.

use std::time::Duration;

use pyo3::prelude::*;
use tokio::sync::{mpsc, oneshot};

use crate::error::Error;

pub(crate) mod bridge_state;
pub(crate) mod command_loop;
mod config;
pub(crate) mod ffi_dispatch;
mod handlers;
pub(crate) mod py_scripts;
pub(crate) mod streaming;
pub(crate) mod venv;

#[cfg(test)]
mod tests;

// Re-export items used by sibling modules and external crate consumers.
pub(crate) use bridge_state::{AgentBridgeState, AgentId, bridge_state, next_agent_id};
pub use config::{BackendLogLevel, RuntimeConfig};
pub(crate) use ffi_dispatch::{
    dispatch_rust_hook, dispatch_rust_policy_confirm, dispatch_rust_tool, initializing_hook_runners,
};

/// Default delay between successive chat commands to prevent burst requests.
pub const DEFAULT_INTER_AGENT_DELAY: Duration = Duration::from_millis(500);

/// Default command channel buffer size.
const DEFAULT_CHANNEL_CAPACITY: usize = 64;

/// Default timeout for joining the Python thread on shutdown.
const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

/// Commands sent from Rust to the Python thread.
///
/// Each variant is constructed in `impl Runtime for PythonRuntime` and
/// dispatched in `command_loop::run_async_command_loop`.
pub(crate) enum PyCommand {
    /// Create a new agent with the given configuration dict as JSON.
    ///
    /// The reply carries both the agent ID and tool definitions discovered
    /// by the Python SDK (Rust tools + MCP tools — builtins are added later).
    CreateAgent {
        agent_id: u64,
        config_json: String,
        reply: oneshot::Sender<Result<(AgentId, Vec<handlers::agent::RawToolInfo>), Error>>,
    },
    /// Send a chat message to an agent.
    Chat {
        agent_id: AgentId,
        prompt: String,
        reply: oneshot::Sender<Result<crate::streaming::ChatResponseHandle, Error>>,
    },
    /// Shut down a specific agent.
    ShutdownAgent {
        agent_id: AgentId,
        reply: oneshot::Sender<Result<(), Error>>,
    },
    /// Cancel active execution on the agent.
    Cancel {
        agent_id: AgentId,
        reply: oneshot::Sender<Result<(), Error>>,
    },
    /// Wait for the agent to stabilize/become idle.
    WaitForIdle {
        agent_id: AgentId,
        reply: oneshot::Sender<Result<(), Error>>,
    },
    /// Send a message without waiting for completion (fire-and-forget).
    Send {
        agent_id: AgentId,
        prompt: String,
        reply: oneshot::Sender<Result<(), Error>>,
    },
    /// Signal that the agent is idle.
    SignalIdle {
        agent_id: AgentId,
        reply: oneshot::Sender<Result<(), Error>>,
    },
    /// Wait for the agent to wake up; returns true if woken, false on timeout.
    WaitForWakeup {
        agent_id: AgentId,
        timeout_secs: f64,
        reply: oneshot::Sender<Result<bool, Error>>,
    },
    /// Shut down the entire Python runtime.
    Shutdown,
    /// Retrieve the conversation's message history.
    GetHistory {
        agent_id: AgentId,
        reply: oneshot::Sender<Result<Vec<crate::types::ConversationMessage>, Error>>,
    },
    /// Return the number of completed turns.
    GetTurnCount {
        agent_id: AgentId,
        reply: oneshot::Sender<Result<u32, Error>>,
    },
    /// Return the number of agents currently live in the runtime registry.
    ///
    /// Runtime-level query (no `agent_id`): counts agents that have been
    /// created but not yet shut down or dropped. Used for observability and
    /// leak detection.
    ///
    /// Constructed by `PythonRuntime::active_agent_count()`.
    GetActiveAgentCount {
        reply: oneshot::Sender<Result<usize, Error>>,
    },
    /// Return cumulative token usage across all turns.
    GetTotalUsage {
        agent_id: AgentId,
        reply: oneshot::Sender<Result<crate::types::UsageMetadata, Error>>,
    },
    /// Return token usage from the most recent turn.
    GetLastTurnUsage {
        agent_id: AgentId,
        reply: oneshot::Sender<Result<crate::types::UsageMetadata, Error>>,
    },
    /// Clear the conversation history.
    ClearHistory {
        agent_id: AgentId,
        reply: oneshot::Sender<Result<(), Error>>,
    },
    /// Remove the last user+model turn pair from conversation history.
    ///
    /// Used for safety recovery: when a model safety-filters trip, removing
    /// the refusal from history gives the model a fresh chance on retry.
    RemoveLastTurn {
        agent_id: AgentId,
        reply: oneshot::Sender<Result<(), Error>>,
    },
    /// Return step indices where compaction occurred.
    GetCompactionIndices {
        agent_id: AgentId,
        reply: oneshot::Sender<Result<Vec<u32>, Error>>,
    },
    /// Return the text of the last model response.
    GetLastResponse {
        agent_id: AgentId,
        reply: oneshot::Sender<Result<Option<String>, Error>>,
    },
    /// Delete the conversation and all associated state.
    ///
    /// Constructed by `impl Runtime for PythonRuntime::delete()` — only
    /// reachable when an external consumer calls `AgentHandle::delete()`.
    Delete {
        agent_id: AgentId,
        reply: oneshot::Sender<Result<(), Error>>,
    },
    /// Disconnect from the agent without deleting state.
    ///
    /// Constructed by `impl Runtime for PythonRuntime::disconnect()`.
    Disconnect {
        agent_id: AgentId,
        reply: oneshot::Sender<Result<(), Error>>,
    },
    /// Check whether the agent is currently idle.
    ///
    /// Constructed by `impl Runtime for PythonRuntime::is_idle()`.
    IsIdle {
        agent_id: AgentId,
        reply: oneshot::Sender<Result<bool, Error>>,
    },
}

/// Manages a dedicated Python thread with an asyncio event loop.
///
/// All Python/SDK interactions go through the command channel. This isolates
/// GIL acquisition to the Python thread and keeps the tokio runtime responsive.
pub struct PythonRuntime {
    cmd_tx: mpsc::Sender<PyCommand>,
    thread: Option<std::thread::JoinHandle<()>>,
    config: RuntimeConfig,
}

impl std::fmt::Debug for PythonRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PythonRuntime")
            .field("config", &self.config)
            .field(
                "thread_running",
                &self.thread.as_ref().is_some_and(|t| !t.is_finished()),
            )
            .finish_non_exhaustive()
    }
}

impl PythonRuntime {
    /// Spawn a new Python runtime on a dedicated thread.
    ///
    /// Creates an asyncio event loop in the thread and starts the command
    /// dispatch loop.
    ///
    /// # Errors
    ///
    /// Returns `Error::BackendError` if the thread fails to spawn or
    /// Python initialization fails.
    pub fn new(config: RuntimeConfig) -> Result<Self, Error> {
        let (cmd_tx, cmd_rx) = mpsc::channel(config.channel_capacity);

        let thread_config = config.clone();
        let thread = std::thread::Builder::new()
            .name("agy-bridge-python-runtime".into())
            .spawn(move || {
                python_thread_main(cmd_rx, &thread_config);
            })
            .map_err(|e| Error::BackendError {
                message: format!("Failed to spawn Python runtime thread: {e}"),
            })?;

        Ok(Self {
            cmd_tx,
            thread: Some(thread),
            config,
        })
    }

    /// Send a command to the Python thread and await the result.
    ///
    /// This is the primary interface for all Python interactions.
    ///
    /// # Errors
    ///
    /// Returns `Error::ChannelClosed` if the Python thread has exited or the
    /// reply channel is dropped before a response is sent.
    async fn send_command<T>(
        &self,
        operation: &str,
        build_cmd: impl FnOnce(oneshot::Sender<Result<T, Error>>) -> PyCommand,
    ) -> Result<T, Error> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let cmd = build_cmd(reply_tx);

        self.cmd_tx
            .send(cmd)
            .await
            .map_err(|e| Error::ChannelClosed {
                message: format!("Python runtime thread has exited (sending {operation}): {e}"),
            })?;

        let result = reply_rx.await.map_err(|e| Error::ChannelClosed {
            message: format!("Reply channel dropped for {operation}: {e}"),
        })??;

        Ok(result)
    }

    /// Return the number of agents currently live in this runtime.
    ///
    /// Counts agents that have been created but not yet shut down or dropped.
    /// Primarily useful for observability and for asserting clean teardown.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ChannelClosed`] if the runtime thread has exited.
    pub(crate) async fn active_agent_count(&self) -> Result<usize, Error> {
        self.send_command("active_agent_count", |reply| {
            PyCommand::GetActiveAgentCount { reply }
        })
        .await
    }

    /// Graceful shutdown: send `Shutdown` command, then join the thread.
    ///
    /// # Errors
    ///
    /// Returns `Error::Timeout` if the thread doesn't join within the
    /// configured shutdown timeout, or `Error::BackendError` if the
    /// thread panicked.
    pub async fn shutdown(mut self) -> Result<(), Error> {
        // Signal the command loop to exit.
        // Ignoring send error: if the receiver is already gone the thread
        // is already exiting, which is the outcome we want.
        if let Err(e) = self.cmd_tx.send(PyCommand::Shutdown).await {
            tracing::warn!("Shutdown command send failed (thread may already be exiting): {e}");
        }

        // Take the JoinHandle so Drop doesn't fire the "dropped without
        // shutdown" warning.
        let Some(thread) = self.thread.take() else {
            tracing::warn!("PythonRuntime::shutdown() called but thread handle already taken");
            return Ok(());
        };

        let shutdown_timeout = self.config.shutdown_timeout;
        let join_result = tokio::time::timeout(
            shutdown_timeout,
            tokio::task::spawn_blocking(move || thread.join()),
        )
        .await;

        match join_result {
            Ok(Ok(Ok(()))) => {
                tracing::info!("Python runtime thread joined successfully");
                Ok(())
            }
            Ok(Ok(Err(panic_payload))) => {
                let panic_msg = panic_payload.downcast_ref::<&str>().map_or_else(
                    || {
                        panic_payload
                            .downcast_ref::<String>()
                            .map_or_else(|| format!("{panic_payload:?}"), Clone::clone)
                    },
                    |s| (*s).to_string(),
                );
                tracing::error!(
                    panic_message = %panic_msg,
                    "Python runtime thread panicked during shutdown"
                );
                Err(Error::BackendError {
                    message: format!("Python runtime thread panicked during shutdown: {panic_msg}"),
                })
            }
            Ok(Err(join_err)) => {
                tracing::error!("spawn_blocking join error: {join_err}");
                Err(Error::BackendError {
                    message: format!("Failed to join Python thread: {join_err}"),
                })
            }
            Err(_elapsed) => {
                tracing::error!(
                    timeout_secs = shutdown_timeout.as_secs(),
                    "Python runtime thread did not exit within shutdown timeout"
                );
                Err(Error::Timeout {
                    duration: shutdown_timeout,
                    operation: "PythonRuntime::shutdown (thread join)".to_string(),
                })
            }
        }
    }
}

impl Drop for PythonRuntime {
    fn drop(&mut self) {
        // If `shutdown()` was already called it took the thread handle, so
        // there is nothing left to clean up.
        let Some(thread) = self.thread.take() else {
            return;
        };

        // Best-effort: prompt the command loop to stop so it runs
        // `cleanup_remaining_agents` (calling `__aexit__` on any still-live
        // agent) and then exits. If the channel buffer is momentarily full
        // this send fails, but `cmd_tx` is dropped immediately after this
        // function returns, which closes the channel and also stops the loop.
        if let Err(e) = self.cmd_tx.try_send(PyCommand::Shutdown) {
            tracing::debug!(
                error = %e,
                "PythonRuntime::drop: could not eagerly signal shutdown; \
                 relying on channel close"
            );
        }

        // Wait — bounded by the configured shutdown timeout — for the Python
        // thread to finish releasing resources. This keeps teardown
        // deterministic (no leaked Python objects) without risking an
        // unbounded block if the thread misbehaves.
        let deadline = std::time::Instant::now() + self.config.shutdown_timeout;
        while !thread.is_finished() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        if thread.is_finished() {
            if thread.join().is_err() {
                tracing::error!("Python runtime thread panicked during drop cleanup");
            } else {
                tracing::debug!("Python runtime thread joined cleanly on drop");
            }
        } else {
            // Dropping `cmd_tx` (right after this returns) closes the channel,
            // so the loop still exits and cleans up; we simply stop blocking
            // the dropping thread past the timeout.
            tracing::warn!(
                "Python runtime thread still running after shutdown timeout during drop — \
                 detaching; agent cleanup will complete asynchronously"
            );
        }
    }
}

/// Entry point for the dedicated Python thread.
fn python_thread_main(cmd_rx: mpsc::Receiver<PyCommand>, config: &RuntimeConfig) {
    Python::initialize();

    // Environment variables are already loaded by load_dotenv() at bridge
    // construction time, before any threads are spawned.

    // Configure sys.path so the venv's site-packages are importable.
    Python::attach(|py| {
        if let Err(e) = venv::configure_python_sys_path(py) {
            tracing::error!(
                error = %e,
                "Failed to configure Python sys.path in runtime thread — \
                 venv imports will likely fail"
            );
        }
    });

    if let Err(e) = run_live_thread(cmd_rx, config) {
        tracing::error!(error = %e, "Python runtime thread failed");
    }

    tracing::info!("Python runtime thread exiting");
}

/// Live SDK thread: creates an asyncio event loop and dispatches commands
/// to the real Antigravity SDK via `pyo3_async_runtimes`.
fn run_live_thread(cmd_rx: mpsc::Receiver<PyCommand>, config: &RuntimeConfig) -> Result<(), Error> {
    Python::attach(|py| {
        let asyncio = py.import("asyncio").map_err(|e| Error::BackendError {
            message: format!("Failed to import asyncio: {e}"),
        })?;
        let event_loop =
            asyncio
                .call_method0("new_event_loop")
                .map_err(|e| Error::BackendError {
                    message: format!("Failed to create new asyncio event loop: {e}"),
                })?;
        asyncio
            .call_method1("set_event_loop", (&event_loop,))
            .map_err(|e| Error::BackendError {
                message: format!("Failed to set asyncio event loop: {e}"),
            })?;

        // Register event_loop in globals for access from any thread
        let sys = py.import("sys").map_err(|e| Error::BackendError {
            message: format!("Failed to import sys: {e}"),
        })?;
        let sys_modules = sys.getattr("modules").map_err(|e| Error::BackendError {
            message: format!("Failed to get sys.modules: {e}"),
        })?;
        let globals_mod = if sys_modules
            .contains(command_loop::AGY_BRIDGE_GLOBALS_MODULE)
            .map_err(|e| Error::BackendError {
                message: format!("Failed to check sys.modules: {e}"),
            })? {
            sys_modules
                .get_item(command_loop::AGY_BRIDGE_GLOBALS_MODULE)
                .map_err(|e| Error::BackendError {
                    message: format!("Failed to get _agy_bridge_globals: {e}"),
                })?
        } else {
            let types = py.import("types").map_err(|e| Error::BackendError {
                message: format!("Failed to import types: {e}"),
            })?;
            let module = types
                .getattr("ModuleType")
                .map_err(|e| Error::BackendError {
                    message: format!("Failed to get ModuleType: {e}"),
                })?
                .call1((command_loop::AGY_BRIDGE_GLOBALS_MODULE,))
                .map_err(|e| Error::BackendError {
                    message: format!("Failed to create ModuleType: {e}"),
                })?;
            sys_modules
                .set_item(command_loop::AGY_BRIDGE_GLOBALS_MODULE, &module)
                .map_err(|e| Error::BackendError {
                    message: format!("Failed to register _agy_bridge_globals: {e}"),
                })?;
            module
        };
        globals_mod
            .setattr("EVENT_LOOP", &event_loop)
            .map_err(|e| Error::BackendError {
                message: format!("Failed to set EVENT_LOOP in globals: {e}"),
            })?;

        tracing::info!("Python asyncio event loop created on runtime thread");

        let inter_agent_delay = config.inter_agent_delay;
        let event_loop_obj = event_loop.clone().unbind();
        let run_fut =
            pyo3_async_runtimes::tokio::run_until_complete(event_loop.clone(), async move {
                command_loop::run_async_command_loop(event_loop_obj, cmd_rx, inter_agent_delay)
                    .await
            });

        if let Err(e) = run_fut {
            // Close the event loop best-effort before propagating.
            if let Err(close_err) = event_loop.call_method0("close") {
                tracing::warn!("Failed to close asyncio event loop: {close_err}");
            }
            return Err(Error::BackendError {
                message: format!("Python runtime command loop failed: {e}"),
            });
        }

        if let Err(e) = event_loop.call_method0("close") {
            tracing::warn!("Failed to close asyncio event loop: {e}");
        }

        Ok(())
    })
}

/// Compute which SDK builtin tools are active based on the agent's
/// [`CapabilitiesConfig`].
///
/// - `enabled_tools: Some(list)` → only those tools are active.
/// - `disabled_tools: Some(list)` → all tools minus the disabled ones.
/// - Neither set → all builtin tools are active.
fn compute_active_builtins(
    config: &crate::config::AgentConfig,
) -> Vec<crate::config::BuiltinTools> {
    let Some(caps) = config.capabilities.as_ref() else {
        return crate::config::BuiltinTools::all_tools().to_vec();
    };

    // `enabled_tools`, when present, is authoritative: an explicit list selects
    // exactly those tools, and an explicit empty list disables all builtins.
    if let Some(enabled) = caps.enabled_tools.as_ref() {
        return enabled.clone();
    }

    // Otherwise, a `disabled_tools` list subtracts from the full builtin set.
    if let Some(disabled) = caps.disabled_tools.as_ref() {
        return crate::config::BuiltinTools::all_tools()
            .iter()
            .filter(|t| !disabled.contains(t))
            .cloned()
            .collect();
    }

    // Neither set → all builtin tools are active.
    crate::config::BuiltinTools::all_tools().to_vec()
}

impl crate::agent::Runtime for PythonRuntime {
    async fn create_agent(
        &self,
        agent_id: u64,
        config: crate::config::AgentConfig,
    ) -> Result<(crate::agent::AgentId, Vec<crate::tools::AvailableTool>), Error> {
        // Serialize the AgentConfig and inject the runtime's backend log
        // level so the Python init script can configure logging without
        // needing a separate FFI parameter.
        let config_json = {
            let mut val = serde_json::to_value(&config).map_err(|e| Error::BackendError {
                message: format!("Failed to serialize AgentConfig: {e}"),
            })?;
            if let serde_json::Value::Object(ref mut map) = val {
                map.insert(
                    "_backend_log_level".to_owned(),
                    serde_json::Value::String(self.config.backend_log_level.as_str().to_owned()),
                );
            }
            serde_json::to_string(&val).map_err(|e| Error::BackendError {
                message: format!("Failed to re-serialize config JSON: {e}"),
            })?
        };

        // Collect the names of custom Rust tools so we can tag them correctly.
        let custom_tool_names: std::collections::HashSet<String> =
            config.tools.iter().map(|t| t.name.clone()).collect();

        let (raw_id, raw_tools) = self
            .send_command("create_agent", |reply| PyCommand::CreateAgent {
                agent_id,
                config_json,
                reply,
            })
            .await?;

        // Compute which builtins are active so we can tag and deduplicate them.
        let active_builtins = compute_active_builtins(&config);
        let builtin_names: std::collections::HashSet<&str> = active_builtins
            .iter()
            .map(crate::config::BuiltinTools::as_sdk_name)
            .collect();

        // Convert RawToolInfo → AvailableTool with source tags.
        // Python's ToolRunner includes builtins in its `tools` dict, so we
        // skip them here and add them back below with the Builtin tag.
        let mut available_tools: Vec<crate::tools::AvailableTool> = raw_tools
            .into_iter()
            .filter(|raw| !builtin_names.contains(raw.name.as_str()))
            .map(|raw| {
                let source = if custom_tool_names.contains(&raw.name) {
                    crate::tools::ToolSource::Custom
                } else {
                    crate::tools::ToolSource::Mcp
                };
                crate::tools::AvailableTool {
                    name: raw.name,
                    description: raw.description,
                    parameter_schema: raw.parameter_schema,
                    source,
                }
            })
            .collect();

        // Add builtin tools with their known descriptions.
        for builtin in active_builtins {
            available_tools.push(crate::tools::AvailableTool {
                name: builtin.as_sdk_name().to_owned(),
                description: builtin.description().to_owned(),
                parameter_schema: serde_json::Value::Null,
                source: crate::tools::ToolSource::Builtin,
            });
        }

        tracing::info!(
            agent_id = raw_id.0,
            tool_count = available_tools.len(),
            tools = ?available_tools.iter().map(|t| format!("{t}")).collect::<Vec<_>>(),
            "Agent created with available tools"
        );

        Ok((raw_id.0, available_tools))
    }

    async fn chat(
        &self,
        agent_id: crate::agent::AgentId,
        content: &crate::content::Content,
    ) -> Result<crate::streaming::ChatResponseHandle, Error> {
        let prompt = match content {
            crate::content::Content::Text { text } => text.clone(),
            other => crate::content::content_to_json(other)?,
        };
        self.send_command("chat", |reply| PyCommand::Chat {
            agent_id: AgentId(agent_id),
            prompt,
            reply,
        })
        .await
    }

    async fn shutdown_agent(&self, agent_id: crate::agent::AgentId) -> Result<(), Error> {
        self.send_command("shutdown_agent", |reply| PyCommand::ShutdownAgent {
            agent_id: AgentId(agent_id),
            reply,
        })
        .await
    }

    fn try_shutdown_agent(&self, agent_id: crate::agent::AgentId) {
        // Fire-and-forget: create a oneshot whose receiver we drop immediately.
        // The Python thread will still process the shutdown; we just don't wait
        // for the result.
        let (reply, _) = oneshot::channel();
        if let Err(e) = self.cmd_tx.try_send(PyCommand::ShutdownAgent {
            agent_id: AgentId(agent_id),
            reply,
        }) {
            tracing::debug!(
                agent_id = agent_id,
                error = %e,
                "try_shutdown_agent: channel send failed (runtime may already be gone)"
            );
        }
    }

    async fn cancel(&self, agent_id: crate::agent::AgentId) -> Result<(), Error> {
        self.send_command("cancel", |reply| PyCommand::Cancel {
            agent_id: AgentId(agent_id),
            reply,
        })
        .await
    }

    async fn wait_for_idle(&self, agent_id: crate::agent::AgentId) -> Result<(), Error> {
        self.send_command("wait_for_idle", |reply| PyCommand::WaitForIdle {
            agent_id: AgentId(agent_id),
            reply,
        })
        .await
    }

    async fn send(
        &self,
        agent_id: crate::agent::AgentId,
        content: &crate::content::Content,
    ) -> Result<(), Error> {
        let prompt = match content {
            crate::content::Content::Text { text } => text.clone(),
            other => crate::content::content_to_json(other)?,
        };
        self.send_command("send", |reply| PyCommand::Send {
            agent_id: AgentId(agent_id),
            prompt,
            reply,
        })
        .await
    }

    async fn signal_idle(&self, agent_id: crate::agent::AgentId) -> Result<(), Error> {
        self.send_command("signal_idle", |reply| PyCommand::SignalIdle {
            agent_id: AgentId(agent_id),
            reply,
        })
        .await
    }

    async fn wait_for_wakeup(
        &self,
        agent_id: crate::agent::AgentId,
        timeout: std::time::Duration,
    ) -> Result<bool, Error> {
        self.send_command("wait_for_wakeup", |reply| PyCommand::WaitForWakeup {
            agent_id: AgentId(agent_id),
            timeout_secs: timeout.as_secs_f64(),
            reply,
        })
        .await
    }

    async fn history(
        &self,
        agent_id: crate::agent::AgentId,
    ) -> Result<Vec<crate::types::ConversationMessage>, Error> {
        self.send_command("get_history", |reply| PyCommand::GetHistory {
            agent_id: AgentId(agent_id),
            reply,
        })
        .await
    }

    async fn turn_count(&self, agent_id: crate::agent::AgentId) -> Result<u32, Error> {
        self.send_command("get_turn_count", |reply| PyCommand::GetTurnCount {
            agent_id: AgentId(agent_id),
            reply,
        })
        .await
    }

    async fn total_usage(
        &self,
        agent_id: crate::agent::AgentId,
    ) -> Result<crate::types::UsageMetadata, Error> {
        self.send_command("get_total_usage", |reply| PyCommand::GetTotalUsage {
            agent_id: AgentId(agent_id),
            reply,
        })
        .await
    }

    async fn last_turn_usage(
        &self,
        agent_id: crate::agent::AgentId,
    ) -> Result<crate::types::UsageMetadata, Error> {
        self.send_command("get_last_turn_usage", |reply| PyCommand::GetLastTurnUsage {
            agent_id: AgentId(agent_id),
            reply,
        })
        .await
    }

    async fn clear_history(&self, agent_id: crate::agent::AgentId) -> Result<(), Error> {
        self.send_command("clear_history", |reply| PyCommand::ClearHistory {
            agent_id: AgentId(agent_id),
            reply,
        })
        .await
    }

    async fn remove_last_turn(&self, agent_id: crate::agent::AgentId) -> Result<(), Error> {
        self.send_command("remove_last_turn", |reply| PyCommand::RemoveLastTurn {
            agent_id: AgentId(agent_id),
            reply,
        })
        .await
    }

    async fn compaction_indices(&self, agent_id: crate::agent::AgentId) -> Result<Vec<u32>, Error> {
        self.send_command("compaction_indices", |reply| {
            PyCommand::GetCompactionIndices {
                agent_id: AgentId(agent_id),
                reply,
            }
        })
        .await
    }

    async fn last_response(
        &self,
        agent_id: crate::agent::AgentId,
    ) -> Result<Option<String>, Error> {
        self.send_command("last_response", |reply| PyCommand::GetLastResponse {
            agent_id: AgentId(agent_id),
            reply,
        })
        .await
    }

    async fn delete(&self, agent_id: crate::agent::AgentId) -> Result<(), Error> {
        self.send_command("delete", |reply| PyCommand::Delete {
            agent_id: AgentId(agent_id),
            reply,
        })
        .await
    }

    async fn disconnect(&self, agent_id: crate::agent::AgentId) -> Result<(), Error> {
        self.send_command("disconnect", |reply| PyCommand::Disconnect {
            agent_id: AgentId(agent_id),
            reply,
        })
        .await
    }

    async fn is_idle(&self, agent_id: crate::agent::AgentId) -> Result<bool, Error> {
        self.send_command("is_idle", |reply| PyCommand::IsIdle {
            agent_id: AgentId(agent_id),
            reply,
        })
        .await
    }
}

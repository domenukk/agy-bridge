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

#[cfg(feature = "python")]
use pyo3::prelude::*;
#[cfg(feature = "python")]
use tokio::sync::{mpsc, oneshot};

use crate::error::Error;

#[cfg(feature = "native")]
pub mod native;
#[cfg(feature = "native")]
pub use native::NativeRuntime;

pub(crate) mod bridge_state;
mod config;

#[cfg(feature = "python")]
pub(crate) mod command_loop;
#[cfg(feature = "python")]
pub(crate) mod ffi_dispatch;
#[cfg(feature = "python")]
mod handlers;
#[cfg(feature = "python")]
pub(crate) mod py_scripts;
#[cfg(feature = "python")]
mod python_runtime_impl;
#[cfg(feature = "python")]
pub(crate) mod streaming;
#[cfg(feature = "python")]
pub(crate) mod venv;

#[cfg(test)]
#[cfg(feature = "python")]
use python_runtime_impl::compute_active_builtins;
#[cfg(test)]
#[cfg(feature = "python")]
mod tests;

// Re-export items used by sibling modules and external crate consumers.
#[cfg(feature = "python")]
pub(crate) use bridge_state::AgentId;
pub(crate) use bridge_state::{AgentBridgeState, bridge_state, initializing_hook_runners};
pub use bridge_state::{
    CapturedToolError, CapturedToolOutput, agent_hooks, agent_policies, agent_policy_handler,
    agent_registry, agent_tool_state, check_tool_execution_allowed, clear_last_tool_error,
    get_agent_conversation_id, next_agent_id, record_last_tool_error, record_last_tool_output,
    remove_agent_bridge_state, set_agent_conversation_id, take_last_tool_error,
    take_last_tool_output,
};
pub use config::{BackendLogLevel, RuntimeConfig};
#[cfg(feature = "python")]
pub(crate) use ffi_dispatch::{
    dispatch_rust_hook, dispatch_rust_policy_confirm, dispatch_rust_tool,
};

pub use crate::agent::{validate_create_config, validate_tool_config};

/// Placeholder [`Runtime`](crate::agent::Runtime) used as [`DefaultRuntime`](crate::DefaultRuntime)
/// when `agy-bridge` is compiled with `--no-default-features` (neither `native`
/// nor `python` enabled).
///
/// Every operation fails with [`Error::RuntimeNotStarted`]. External runtime
/// crates construct [`AgyBridge::with_runtime`](crate::AgyBridge::with_runtime)
/// with their own [`Runtime`](crate::agent::Runtime) implementation instead.
#[derive(Debug, Clone, Copy, Default)]
pub struct UnconfiguredRuntime;

impl UnconfiguredRuntime {
    /// Returns `false` because no backend runtime has been started.
    #[must_use]
    pub const fn is_alive(&self) -> bool {
        false
    }

    /// Always fails with [`Error::RuntimeNotStarted`] when no backend is active.
    pub fn active_agent_count(
        &self,
    ) -> impl std::future::Future<Output = Result<usize, Error>> + Send {
        std::future::ready(Err(Error::RuntimeNotStarted))
    }
}

impl crate::agent::Runtime for UnconfiguredRuntime {
    async fn create_agent(
        &self,
        _agent_id: u64,
        _config: crate::config::AgentConfig,
    ) -> Result<(crate::agent::AgentId, Vec<crate::tools::AvailableTool>), Error> {
        Err(Error::RuntimeNotStarted)
    }

    async fn chat(
        &self,
        _agent_id: crate::agent::AgentId,
        _content: &crate::content::Content,
    ) -> Result<crate::streaming::ChatResponseHandle, Error> {
        Err(Error::RuntimeNotStarted)
    }

    async fn shutdown_agent(&self, _agent_id: crate::agent::AgentId) -> Result<(), Error> {
        Err(Error::RuntimeNotStarted)
    }

    async fn cancel(&self, _agent_id: crate::agent::AgentId) -> Result<(), Error> {
        Err(Error::RuntimeNotStarted)
    }

    async fn wait_for_idle(&self, _agent_id: crate::agent::AgentId) -> Result<(), Error> {
        Err(Error::RuntimeNotStarted)
    }

    async fn send(
        &self,
        _agent_id: crate::agent::AgentId,
        _content: &crate::content::Content,
    ) -> Result<(), Error> {
        Err(Error::RuntimeNotStarted)
    }

    async fn signal_idle(&self, _agent_id: crate::agent::AgentId) -> Result<(), Error> {
        Err(Error::RuntimeNotStarted)
    }

    async fn wait_for_wakeup(
        &self,
        _agent_id: crate::agent::AgentId,
        _timeout: Duration,
    ) -> Result<bool, Error> {
        Err(Error::RuntimeNotStarted)
    }

    async fn history(
        &self,
        _agent_id: crate::agent::AgentId,
    ) -> Result<Vec<crate::types::ConversationMessage>, Error> {
        Err(Error::RuntimeNotStarted)
    }

    async fn turn_count(&self, _agent_id: crate::agent::AgentId) -> Result<u32, Error> {
        Err(Error::RuntimeNotStarted)
    }

    async fn total_usage(
        &self,
        _agent_id: crate::agent::AgentId,
    ) -> Result<crate::types::UsageMetadata, Error> {
        Err(Error::RuntimeNotStarted)
    }

    async fn last_turn_usage(
        &self,
        _agent_id: crate::agent::AgentId,
    ) -> Result<crate::types::UsageMetadata, Error> {
        Err(Error::RuntimeNotStarted)
    }

    async fn clear_history(&self, _agent_id: crate::agent::AgentId) -> Result<(), Error> {
        Err(Error::RuntimeNotStarted)
    }

    async fn last_response(
        &self,
        _agent_id: crate::agent::AgentId,
    ) -> Result<Option<String>, Error> {
        Err(Error::RuntimeNotStarted)
    }

    async fn compaction_indices(
        &self,
        _agent_id: crate::agent::AgentId,
    ) -> Result<Vec<u32>, Error> {
        Err(Error::RuntimeNotStarted)
    }

    async fn delete(&self, _agent_id: crate::agent::AgentId) -> Result<(), Error> {
        Err(Error::RuntimeNotStarted)
    }

    async fn disconnect(&self, _agent_id: crate::agent::AgentId) -> Result<(), Error> {
        Err(Error::RuntimeNotStarted)
    }

    async fn is_idle(&self, _agent_id: crate::agent::AgentId) -> Result<bool, Error> {
        Err(Error::RuntimeNotStarted)
    }

    async fn sandbox_status(
        &self,
        _agent_id: crate::agent::AgentId,
    ) -> Result<Option<crate::types::SandboxStatus>, Error> {
        Err(Error::RuntimeNotStarted)
    }
}

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
#[cfg(feature = "python")]
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
    /// Query the OS command sandbox status reported by the harness.
    ///
    /// Constructed by `impl Runtime for PythonRuntime::sandbox_status()`.
    GetSandboxStatus {
        agent_id: AgentId,
        reply: oneshot::Sender<Result<Option<crate::types::SandboxStatus>, Error>>,
    },
}

/// Manages a dedicated Python thread with an asyncio event loop.
///
/// All Python/SDK interactions go through the command channel. This isolates
/// GIL acquisition to the Python thread and keeps the tokio runtime responsive.
#[cfg(feature = "python")]
pub struct PythonRuntime {
    cmd_tx: Option<mpsc::Sender<PyCommand>>,
    thread: Option<std::thread::JoinHandle<()>>,
    config: RuntimeConfig,
}

#[cfg(feature = "python")]
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

#[cfg(feature = "python")]
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
        ensure_python_initialized()?;

        let (cmd_tx, cmd_rx) = mpsc::channel(config.channel_capacity);
        let (startup_tx, startup_rx) = std::sync::mpsc::sync_channel(1);

        let thread_config = config.clone();
        let thread = std::thread::Builder::new()
            .name("agy-bridge-python-runtime".into())
            .spawn(move || {
                python_thread_main(cmd_rx, &thread_config, &startup_tx);
            })
            .map_err(|e| Error::BackendError {
                message: format!("Failed to spawn Python runtime thread: {e}"),
            })?;

        match startup_rx.recv_timeout(std::time::Duration::from_secs(30)) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(e),
            Err(e) => {
                return Err(Error::BackendError {
                    message: format!(
                        "Python runtime thread failed to signal startup readiness: {e}"
                    ),
                });
            }
        }

        Ok(Self {
            cmd_tx: Some(cmd_tx),
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
        let Some(ref tx) = self.cmd_tx else {
            return Err(Error::ChannelClosed {
                message: format!("Python runtime thread is shut down (sending {operation})"),
            });
        };

        let (reply_tx, reply_rx) = oneshot::channel();
        let cmd = build_cmd(reply_tx);

        tx.send(cmd).await.map_err(|e| Error::ChannelClosed {
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
        if let Some(tx) = self.cmd_tx.take()
            && let Err(e) = tx.send(PyCommand::Shutdown).await
        {
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

#[cfg(feature = "python")]
impl Drop for PythonRuntime {
    fn drop(&mut self) {
        // If `shutdown()` was already called it took the thread handle, so
        // there is nothing left to clean up.
        let Some(tx) = self.cmd_tx.take() else {
            return;
        };
        let Some(thread) = self.thread.take() else {
            return;
        };

        if !thread.is_finished()
            && let Err(e) = tx.try_send(PyCommand::Shutdown)
        {
            tracing::debug!(
                error = %e,
                "PythonRuntime::drop: could not eagerly signal shutdown; \
                 relying on channel close"
            );
        }
        // Explicitly drop sender so the receiver encounters EOF immediately.
        drop(tx);

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
            tracing::warn!(
                "Python runtime thread still running after shutdown timeout during drop — \
                 detaching; agent cleanup will complete asynchronously"
            );
        }
    }
}

/// Initialize `CPython` and import `threading`/`asyncio` on a dedicated bootstrap
/// thread so that `threading.main_thread()` is never an `agy-bridge-python-runtime`
/// worker thread (preventing `ProactorEventLoop` on Windows from calling
/// `signal.set_wakeup_fd` on non-main threads).
#[cfg(feature = "python")]
fn ensure_python_initialized() -> Result<(), Error> {
    static PYTHON_INIT: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
    let res = PYTHON_INIT.get_or_init(|| {
        let handle = std::thread::Builder::new()
            .name("agy-bridge-py-init".into())
            .spawn(|| -> Result<(), String> {
                Python::initialize();
                Python::attach(|py| -> Result<(), String> {
                    let threading = py.import("threading").map_err(|e| e.to_string())?;
                    let current = threading
                        .call_method0("current_thread")
                        .map_err(|e| e.to_string())?;
                    threading
                        .setattr("_main_thread", current)
                        .map_err(|e| e.to_string())?;
                    py.import("asyncio").map_err(|e| e.to_string())?;
                    Ok(())
                })
            })
            .map_err(|e| format!("Failed to spawn Python init thread: {e}"))?;
        handle
            .join()
            .map_err(|panic_err| format!("Python init thread panicked: {panic_err:?}"))?
    });
    match res {
        Ok(()) => Ok(()),
        Err(msg) => Err(Error::BackendError {
            message: msg.clone(),
        }),
    }
}

/// Entry point for the dedicated Python thread.
#[cfg(feature = "python")]
fn python_thread_main(
    cmd_rx: mpsc::Receiver<PyCommand>,
    config: &RuntimeConfig,
    startup_tx: &std::sync::mpsc::SyncSender<Result<(), Error>>,
) {
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

    if let Err(e) = run_live_thread(cmd_rx, config, startup_tx.clone()) {
        tracing::error!(error = %e, "Python runtime thread failed");
        if let Err(send_err) = startup_tx.try_send(Err(e)) {
            tracing::debug!(error = %send_err, "Startup error channel already closed or full");
        }
    }

    tracing::info!("Python runtime thread exiting");
}

#[cfg(feature = "python")]
fn create_event_loop_with_retry(py: Python<'_>) -> Result<Bound<'_, PyAny>, Error> {
    const MAX_EVENT_LOOP_ATTEMPTS: u32 = 5;
    const EVENT_LOOP_RETRY_BACKOFF_MS: u64 = 50;

    let asyncio = py.import("asyncio").map_err(|e| Error::BackendError {
        message: format!("Failed to import asyncio: {e}"),
    })?;
    let mut loop_attempts = 0u32;
    let event_loop = loop {
        match asyncio.call_method0("new_event_loop") {
            Ok(loop_obj) => break loop_obj,
            Err(e) if loop_attempts < MAX_EVENT_LOOP_ATTEMPTS => {
                loop_attempts += 1;
                tracing::warn!(
                    attempt = loop_attempts,
                    error = %e,
                    "asyncio.new_event_loop() failed; retrying"
                );
                std::thread::sleep(std::time::Duration::from_millis(
                    EVENT_LOOP_RETRY_BACKOFF_MS * u64::from(loop_attempts),
                ));
            }
            Err(e) => {
                return Err(Error::BackendError {
                    message: format!("Failed to create new asyncio event loop: {e}"),
                });
            }
        }
    };
    asyncio
        .call_method1("set_event_loop", (&event_loop,))
        .map_err(|e| Error::BackendError {
            message: format!("Failed to set asyncio event loop: {e}"),
        })?;
    Ok(event_loop)
}

/// Live SDK thread: creates an asyncio event loop and dispatches commands
/// to the real Antigravity SDK via `pyo3_async_runtimes`.
#[cfg(feature = "python")]
fn run_live_thread(
    cmd_rx: mpsc::Receiver<PyCommand>,
    config: &RuntimeConfig,
    startup_tx: std::sync::mpsc::SyncSender<Result<(), Error>>,
) -> Result<(), Error> {
    Python::attach(|py| {
        let event_loop = create_event_loop_with_retry(py)?;

        // Register the event loop in the process-global helper module so it can
        // be resolved from any thread. It is stored both as the legacy single
        // `EVENT_LOOP` attribute and (below) in a per-runtime-thread map, so
        // that multiple bridges in one process never clobber each other's loop.
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
        register_thread_event_loop(py, &globals_mod, &event_loop)?;

        tracing::info!("Python asyncio event loop created on runtime thread");

        let inter_agent_delay = config.inter_agent_delay;
        let stream_limits = streaming::StreamLimits::from_config(config);
        let shutdown_timeout = config.shutdown_timeout;
        let event_loop_obj = event_loop.clone().unbind();
        let run_fut =
            pyo3_async_runtimes::tokio::run_until_complete(event_loop.clone(), async move {
                command_loop::run_async_command_loop(
                    event_loop_obj,
                    cmd_rx,
                    inter_agent_delay,
                    stream_limits,
                    shutdown_timeout,
                    startup_tx,
                )
                .await
            });

        unregister_thread_event_loop(py, &globals_mod);

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

/// Register `event_loop` in the process-global `_agy_bridge_globals.EVENT_LOOPS`
/// map, keyed by the current (runtime) thread's Python identity.
///
/// Each [`AgyBridge`](crate::AgyBridge) runs its own runtime thread with its own
/// asyncio loop. A single shared `EVENT_LOOP` attribute would let a later bridge
/// overwrite an earlier one's loop; keying by `threading.get_ident()` keeps them
/// isolated. This map is the per-runtime successor to the legacy `EVENT_LOOP`
/// attribute and is read by the fallback path in `agent_init.py`, which runs on
/// this same runtime thread (so the idents match).
#[cfg(feature = "python")]
fn register_thread_event_loop(
    py: Python<'_>,
    globals_mod: &Bound<'_, PyAny>,
    event_loop: &Bound<'_, PyAny>,
) -> Result<(), Error> {
    let threading = py.import("threading").map_err(|e| Error::BackendError {
        message: format!("Failed to import threading for event-loop registration: {e}"),
    })?;
    let thread_id = threading
        .call_method0("get_ident")
        .map_err(|e| Error::BackendError {
            message: format!("Failed to read threading.get_ident(): {e}"),
        })?;

    let loops = if globals_mod
        .hasattr("EVENT_LOOPS")
        .map_err(|e| Error::BackendError {
            message: format!("Failed to check for EVENT_LOOPS attribute: {e}"),
        })? {
        globals_mod
            .getattr("EVENT_LOOPS")
            .map_err(|e| Error::BackendError {
                message: format!("Failed to get EVENT_LOOPS map: {e}"),
            })?
    } else {
        let dict = pyo3::types::PyDict::new(py).into_any();
        globals_mod
            .setattr("EVENT_LOOPS", &dict)
            .map_err(|e| Error::BackendError {
                message: format!("Failed to create EVENT_LOOPS map: {e}"),
            })?;
        dict
    };

    loops
        .set_item(thread_id, event_loop)
        .map_err(|e| Error::BackendError {
            message: format!("Failed to register runtime event loop by thread id: {e}"),
        })?;
    Ok(())
}

#[cfg(feature = "python")]
fn unregister_thread_event_loop(py: Python<'_>, globals_mod: &Bound<'_, PyAny>) {
    let unregister_res = (|| -> PyResult<()> {
        let threading = py.import("threading")?;
        let thread_id = threading.call_method0("get_ident")?;
        if globals_mod.hasattr("EVENT_LOOPS")? {
            let loops = globals_mod.getattr("EVENT_LOOPS")?;
            let dict = loops.cast::<pyo3::types::PyDict>()?;
            dict.del_item(thread_id)?;
        }
        Ok(())
    })();
    if let Err(e) = unregister_res {
        tracing::debug!(error = %e, "Failed to unregister thread event loop on teardown");
    }
}

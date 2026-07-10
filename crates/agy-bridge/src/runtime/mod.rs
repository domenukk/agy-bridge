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

use std::{sync::Arc, time::Duration};

use pyo3::prelude::*;
use tokio::sync::{mpsc, oneshot};

use crate::{error::Error, quota::QuotaState};

pub(crate) mod bridge_state;
pub(crate) mod command_loop;
pub(crate) mod ffi_dispatch;
mod handlers;
pub(crate) mod py_scripts;
pub(crate) mod streaming;
pub(crate) mod venv;

// Re-export items used by sibling modules and external crate consumers.
pub(crate) use bridge_state::{AgentBridgeState, AgentId, bridge_state};
pub(crate) use ffi_dispatch::{
    CREATE_AGENT_HOOK_GUARD, INITIALIZING_HOOK_RUNNER, dispatch_rust_hook,
    dispatch_rust_policy_confirm, dispatch_rust_tool,
};

/// Safety-net timeout for a single `send_command` round-trip.
///
/// This is the *outer* Rust-side timeout that wraps all commands sent to the
/// Python thread (chat, `create_agent`, cancel, `get_history`, …).  The Python
/// side applies its own, tighter timeouts (`chat_timeout`, `HANDLER_TIMEOUT`),
/// so this value should only fire if the Python thread is completely stuck.
///
/// Defaults to `chat_timeout + 2 minutes` to give inner timeouts room to
/// fire first.
#[must_use]
pub fn default_operation_timeout(chat_timeout: Duration) -> Duration {
    chat_timeout + Duration::from_mins(2)
}
/// Default timeout (seconds) for a single `agent.chat()` round-trip.
/// 120s (2 min) is generous for a normal turn while detecting stalls quickly.
pub const DEFAULT_CHAT_TIMEOUT_SECS: u64 = 120;

/// Default delay between successive chat commands to prevent burst requests.
pub const DEFAULT_INTER_AGENT_DELAY: Duration = Duration::from_millis(500);

/// Default command channel buffer size.
const DEFAULT_CHANNEL_CAPACITY: usize = 64;

/// Default timeout for joining the Python thread on shutdown.
const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

/// Returns the default chat round-trip timeout, configurable via
/// `AGI_CHAT_TIMEOUT_SECS` (defaults to 120 s).
#[must_use]
pub fn default_chat_timeout() -> Duration {
    let secs = std::env::var("AGI_CHAT_TIMEOUT_SECS").map_or(DEFAULT_CHAT_TIMEOUT_SECS, |val| {
        val.parse::<u64>().unwrap_or_else(|e| {
            tracing::warn!(
                value = %val,
                error = %e,
                "Invalid AGI_CHAT_TIMEOUT_SECS, using default {DEFAULT_CHAT_TIMEOUT_SECS}s"
            );
            DEFAULT_CHAT_TIMEOUT_SECS
        })
    });
    Duration::from_secs(secs)
}

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
}

/// Log verbosity for the agent backend runtime.
///
/// Controls the logging level of the underlying agent runtime. This is
/// intentionally backend-agnostic — consumers should not need to know
/// the implementation details of the runtime layer.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackendLogLevel {
    /// Errors only.
    Error,
    /// Warnings and errors (default — matches upstream SDK behavior).
    #[default]
    Warn,
    /// Informational messages (verbose — includes raw protocol traffic).
    Info,
    /// Full debug output.
    Debug,
}

impl BackendLogLevel {
    /// Return the lowercase string representation used by the Python side.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
        }
    }
}

impl std::fmt::Display for BackendLogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Configuration for the bridge runtime.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct RuntimeConfig {
    /// Channel buffer size for the command channel.
    pub channel_capacity: usize,
    /// Timeout for individual runtime operations.
    pub operation_timeout: Duration,
    /// Timeout for joining the Python thread on shutdown.
    pub shutdown_timeout: Duration,
    /// Timeout for a single `agent.chat()` round-trip.
    ///
    /// Defaults to the value of `AGI_CHAT_TIMEOUT_SECS` (env var), or 120 s.
    pub chat_timeout: Duration,
    /// Delay injected between successive chat commands to prevent burst requests.
    pub inter_agent_delay: Duration,
    /// Backend runtime log verbosity.
    ///
    /// Defaults to `Warn`, matching the upstream SDK's default behavior.
    /// Set to `Info` or `Debug` for verbose protocol-level diagnostics.
    pub backend_log_level: BackendLogLevel,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        let chat_timeout = default_chat_timeout();
        Self {
            channel_capacity: DEFAULT_CHANNEL_CAPACITY,
            operation_timeout: default_operation_timeout(chat_timeout),
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
            chat_timeout,
            inter_agent_delay: DEFAULT_INTER_AGENT_DELAY,
            backend_log_level: BackendLogLevel::default(),
        }
    }
}

/// Manages a dedicated Python thread with an asyncio event loop.
///
/// All Python/SDK interactions go through the command channel. This isolates
/// GIL acquisition to the Python thread and keeps the tokio runtime responsive.
pub struct PythonRuntime {
    cmd_tx: mpsc::Sender<PyCommand>,
    thread: Option<std::thread::JoinHandle<()>>,
    config: RuntimeConfig,
    /// Per-runtime quota registry. Each API key gets its own [`QuotaState`],
    /// and different `PythonRuntime` instances are fully independent.
    quota_registry: crate::quota::QuotaRegistry,
    /// Default quota state used by `send_command` for runtime-level backoff.
    quota_state: Arc<QuotaState>,
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

        let quota_registry = crate::quota::QuotaRegistry::new();
        let quota_state = quota_registry.state_for_key("");
        Ok(Self {
            cmd_tx,
            thread: Some(thread),
            config,
            quota_registry,
            quota_state,
        })
    }

    /// Send a command to the Python thread and await the result.
    ///
    /// This is the primary interface for all Python interactions. It checks
    /// quota state before sending and applies a configurable timeout.
    ///
    /// # Errors
    ///
    /// Returns `Error::ChannelClosed` if the Python thread has exited,
    /// `Error::Timeout` if the operation exceeds the configured timeout.
    async fn send_command<T>(
        &self,
        operation: &str,
        is_llm_op: bool,
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

        let result = crate::error::with_timeout(self.config.operation_timeout, operation, async {
            reply_rx.await.map_err(|e| Error::ChannelClosed {
                message: format!("Reply channel dropped for {operation}: {e}"),
            })?
        })
        .await?;

        // Only reset quota backoff for LLM operations (e.g. chat); non-LLM
        // ops succeeding should not clear a 429 backoff.
        if is_llm_op {
            self.quota_state.record_success();
        }

        Ok(result)
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

    /// Access the shared quota state.
    #[must_use]
    pub const fn quota_state(&self) -> &Arc<QuotaState> {
        &self.quota_state
    }
}

impl Drop for PythonRuntime {
    fn drop(&mut self) {
        if self.thread.is_some() {
            tracing::warn!(
                "PythonRuntime dropped without calling shutdown() — \
                 Python thread may still be running"
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

        let chat_timeout = config.chat_timeout;
        let inter_agent_delay = config.inter_agent_delay;
        let event_loop_obj = event_loop.clone().unbind();
        let run_fut =
            pyo3_async_runtimes::tokio::run_until_complete(event_loop.clone(), async move {
                command_loop::run_async_command_loop(
                    event_loop_obj,
                    cmd_rx,
                    chat_timeout,
                    inter_agent_delay,
                )
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
    match config.capabilities.as_ref() {
        Some(caps) if caps.enabled_tools.as_ref().is_some_and(|v| !v.is_empty()) => {
            caps.enabled_tools.clone().unwrap_or_default()
        }
        Some(caps) if caps.enabled_tools.as_ref().is_some_and(Vec::is_empty) => {
            // Explicitly empty = no builtins
            Vec::new()
        }
        Some(caps) if caps.disabled_tools.is_some() => {
            let disabled = caps.disabled_tools.as_ref().unwrap();
            crate::config::BuiltinTools::all_tools()
                .iter()
                .filter(|t| !disabled.contains(t))
                .cloned()
                .collect()
        }
        _ => crate::config::BuiltinTools::all_tools().to_vec(),
    }
}

impl crate::agent::Runtime for PythonRuntime {
    async fn create_agent(
        &self,
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
            .send_command("create_agent", false, |reply| PyCommand::CreateAgent {
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
        self.send_command("chat", true, |reply| PyCommand::Chat {
            agent_id: AgentId(agent_id),
            prompt,
            reply,
        })
        .await
    }

    async fn shutdown_agent(&self, agent_id: crate::agent::AgentId) -> Result<(), Error> {
        self.send_command("shutdown_agent", false, |reply| PyCommand::ShutdownAgent {
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
        self.send_command("cancel", false, |reply| PyCommand::Cancel {
            agent_id: AgentId(agent_id),
            reply,
        })
        .await
    }

    async fn wait_for_idle(&self, agent_id: crate::agent::AgentId) -> Result<(), Error> {
        self.send_command("wait_for_idle", false, |reply| PyCommand::WaitForIdle {
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
        self.send_command("send", false, |reply| PyCommand::Send {
            agent_id: AgentId(agent_id),
            prompt,
            reply,
        })
        .await
    }

    async fn signal_idle(&self, agent_id: crate::agent::AgentId) -> Result<(), Error> {
        self.send_command("signal_idle", false, |reply| PyCommand::SignalIdle {
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
        self.send_command("wait_for_wakeup", false, |reply| PyCommand::WaitForWakeup {
            agent_id: AgentId(agent_id),
            timeout_secs: timeout.as_secs_f64(),
            reply,
        })
        .await
    }

    async fn wait_for_quota(&self) {
        self.quota_state.wait_for_quota().await;
    }

    async fn record_quota_hit(&self, retry_after: std::time::Duration) {
        self.quota_state.record_quota_hit(retry_after);
    }

    fn quota_registry(&self) -> &crate::quota::QuotaRegistry {
        &self.quota_registry
    }

    async fn history(
        &self,
        agent_id: crate::agent::AgentId,
    ) -> Result<Vec<crate::types::ConversationMessage>, Error> {
        self.send_command("get_history", false, |reply| PyCommand::GetHistory {
            agent_id: AgentId(agent_id),
            reply,
        })
        .await
    }

    async fn turn_count(&self, agent_id: crate::agent::AgentId) -> Result<u32, Error> {
        self.send_command("get_turn_count", false, |reply| PyCommand::GetTurnCount {
            agent_id: AgentId(agent_id),
            reply,
        })
        .await
    }

    async fn total_usage(
        &self,
        agent_id: crate::agent::AgentId,
    ) -> Result<crate::types::UsageMetadata, Error> {
        self.send_command("get_total_usage", false, |reply| PyCommand::GetTotalUsage {
            agent_id: AgentId(agent_id),
            reply,
        })
        .await
    }

    async fn last_turn_usage(
        &self,
        agent_id: crate::agent::AgentId,
    ) -> Result<crate::types::UsageMetadata, Error> {
        self.send_command("get_last_turn_usage", false, |reply| {
            PyCommand::GetLastTurnUsage {
                agent_id: AgentId(agent_id),
                reply,
            }
        })
        .await
    }

    async fn clear_history(&self, agent_id: crate::agent::AgentId) -> Result<(), Error> {
        self.send_command("clear_history", false, |reply| PyCommand::ClearHistory {
            agent_id: AgentId(agent_id),
            reply,
        })
        .await
    }

    async fn compaction_indices(&self, agent_id: crate::agent::AgentId) -> Result<Vec<u32>, Error> {
        self.send_command("compaction_indices", false, |reply| {
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
        self.send_command("last_response", false, |reply| PyCommand::GetLastResponse {
            agent_id: AgentId(agent_id),
            reply,
        })
        .await
    }

    async fn delete(&self, agent_id: crate::agent::AgentId) -> Result<(), Error> {
        self.send_command("delete", false, |reply| PyCommand::Delete {
            agent_id: AgentId(agent_id),
            reply,
        })
        .await
    }

    async fn disconnect(&self, agent_id: crate::agent::AgentId) -> Result<(), Error> {
        self.send_command("disconnect", false, |reply| PyCommand::Disconnect {
            agent_id: AgentId(agent_id),
            reply,
        })
        .await
    }

    async fn is_idle(&self, agent_id: crate::agent::AgentId) -> Result<bool, Error> {
        self.send_command("is_idle", false, |reply| PyCommand::IsIdle {
            agent_id: AgentId(agent_id),
            reply,
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{ffi_dispatch::check_tool_execution_allowed, *};

    fn test_config() -> RuntimeConfig {
        RuntimeConfig {
            channel_capacity: 16,
            operation_timeout: Duration::from_secs(10),
            shutdown_timeout: Duration::from_secs(5),
            chat_timeout: Duration::from_mins(1),
            inter_agent_delay: Duration::from_millis(100),
            backend_log_level: BackendLogLevel::default(),
        }
    }

    #[tokio::test]
    async fn test_runtime_creation_and_shutdown() {
        // Shutdown should complete cleanly.
        PythonRuntime::new(test_config())
            .expect("Failed to create runtime")
            .shutdown()
            .await
            .expect("Shutdown failed");
    }

    #[test]
    fn runtime_config_serde_roundtrip() {
        let config = test_config();
        let json = serde_json::to_string(&config).unwrap();
        let parsed: RuntimeConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.channel_capacity, 16);
        assert_eq!(parsed.operation_timeout, Duration::from_secs(10));
        assert_eq!(parsed.shutdown_timeout, Duration::from_secs(5));
        assert_eq!(parsed.chat_timeout, Duration::from_mins(1));
        assert_eq!(parsed.inter_agent_delay, Duration::from_millis(100));
        assert_eq!(parsed.backend_log_level, BackendLogLevel::Warn);
    }

    #[test]
    fn backend_log_level_default_is_warn() {
        assert_eq!(BackendLogLevel::default(), BackendLogLevel::Warn);
    }

    #[test]
    fn backend_log_level_serde_roundtrip_all_variants() {
        for (variant, expected_str) in [
            (BackendLogLevel::Error, "\"error\""),
            (BackendLogLevel::Warn, "\"warn\""),
            (BackendLogLevel::Info, "\"info\""),
            (BackendLogLevel::Debug, "\"debug\""),
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(json, expected_str, "serialize {variant:?}");
            let parsed: BackendLogLevel = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, variant, "roundtrip {variant:?}");
        }
    }

    #[test]
    fn backend_log_level_as_str() {
        assert_eq!(BackendLogLevel::Error.as_str(), "error");
        assert_eq!(BackendLogLevel::Warn.as_str(), "warn");
        assert_eq!(BackendLogLevel::Info.as_str(), "info");
        assert_eq!(BackendLogLevel::Debug.as_str(), "debug");
    }

    #[test]
    fn backend_log_level_display() {
        assert_eq!(format!("{}", BackendLogLevel::Error), "error");
        assert_eq!(format!("{}", BackendLogLevel::Warn), "warn");
        assert_eq!(format!("{}", BackendLogLevel::Info), "info");
        assert_eq!(format!("{}", BackendLogLevel::Debug), "debug");
    }

    #[test]
    fn runtime_config_with_custom_backend_log_level() {
        let config = RuntimeConfig {
            backend_log_level: BackendLogLevel::Debug,
            ..test_config()
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: RuntimeConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.backend_log_level, BackendLogLevel::Debug);
    }

    #[test]
    fn default_operation_timeout_is_chat_plus_margin() {
        let config = RuntimeConfig::default();
        let expected = config.chat_timeout + Duration::from_mins(2);
        assert_eq!(
            config.operation_timeout, expected,
            "operation_timeout should be chat_timeout + 2min safety margin"
        );
    }

    #[test]
    fn stop_candidate_exception_is_backend_error() {
        Python::initialize();
        Python::attach(|py| {
            let globals = pyo3::types::PyDict::new(py);
            py.run(
                c"
class StopCandidateException(Exception):
    pass
err = StopCandidateException(\"dummy\")
",
                Some(&globals),
                None,
            )
            .unwrap();

            let err_obj = globals.get_item("err").unwrap().unwrap();
            let err = PyErr::from_value(err_obj);

            let mapped = crate::error::classify_py_error(py, &err);

            assert!(
                matches!(mapped, crate::error::Error::BackendError { .. }),
                "StopCandidateException should be classified as BackendError, got: {mapped:?}"
            );
        });
    }

    #[test]
    fn max_tokens_exception_is_backend_error() {
        Python::initialize();
        Python::attach(|py| {
            let globals = pyo3::types::PyDict::new(py);
            py.run(
                c"
class MaxTokensException(Exception):
    pass
err = MaxTokensException(\"dummy\")
",
                Some(&globals),
                None,
            )
            .unwrap();

            let err_obj = globals.get_item("err").unwrap().unwrap();
            let err = PyErr::from_value(err_obj);

            let mapped = crate::error::classify_py_error(py, &err);

            assert!(
                matches!(mapped, crate::error::Error::BackendError { .. }),
                "MaxTokensException should be classified as BackendError, got: {mapped:?}"
            );
        });
    }

    struct MockAskUserHandler {
        should_allow: std::sync::atomic::AtomicBool,
    }

    impl crate::policies::AskUserHandler for MockAskUserHandler {
        fn confirm(&self, _tool_name: &str, _tool_args: &serde_json::Value) -> bool {
            self.should_allow.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    #[test]
    fn test_ask_user_policy_custom_tool_gating() {
        let agent_id: u64 = 999;

        // 1. Setup the PolicySet with an AskUser rule for "dangerous_tool"
        let mut policies = crate::policies::PolicySet::new();
        policies
            .push(crate::policies::PolicyRule::AskUser {
                tool: "dangerous_tool".to_owned(),
                handler_id: "confirm_handler".to_owned(),
            })
            .unwrap();

        // 2. Setup mock handler
        let handler = Arc::new(MockAskUserHandler {
            should_allow: std::sync::atomic::AtomicBool::new(true),
        });

        // 3. Mock the tool registry
        let mut registry = crate::tools::ToolRegistry::new();

        /// A dangerous tool.
        #[crate::llm_tool]
        fn dangerous_tool() -> Result<String, String> {
            Ok("Executed dangerous action!".to_owned())
        }
        registry.register(DangerousTool);

        // 4. Register all state in a single bridge_state() insertion
        bridge_state().write().unwrap().insert(
            agent_id,
            AgentBridgeState {
                registry: Some(Arc::new(registry)),
                hook_runner: None,
                policies,
                policy_handler: Some(
                    Arc::clone(&handler) as Arc<dyn crate::policies::AskUserHandler>
                ),
                tool_state: Arc::new(std::sync::RwLock::new(HashMap::new())),
            },
        );

        // 5. Simulate check_tool_execution_allowed when the AskUserHandler allows it (returns true)
        handler
            .should_allow
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let res = check_tool_execution_allowed(agent_id, "dangerous_tool", "{}");
        assert!(res.is_ok(), "Check should succeed");
        assert!(
            res.unwrap(),
            "Should allow tool execution when handler returns true"
        );

        // 6. Simulate check_tool_execution_allowed when the AskUserHandler denies it (returns false)
        handler
            .should_allow
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let res = check_tool_execution_allowed(agent_id, "dangerous_tool", "{}");
        assert!(res.is_ok(), "Check should succeed");
        assert!(
            !res.unwrap(),
            "Should block tool execution when handler returns false"
        );

        // Clean up
        bridge_state().write().unwrap().remove(&agent_id);
    }

    // ── compute_active_builtins tests ─────────────────────────────────

    #[test]
    fn builtins_default_config_returns_all() {
        let config = crate::config::AgentConfig::default();
        let builtins = super::compute_active_builtins(&config);
        assert_eq!(
            builtins.len(),
            crate::config::BuiltinTools::all_tools().len(),
            "default config should produce all builtins"
        );
    }

    #[test]
    fn builtins_no_capabilities_returns_all() {
        let config = crate::config::AgentConfig {
            capabilities: None,
            ..crate::config::AgentConfig::default()
        };
        let builtins = super::compute_active_builtins(&config);
        assert_eq!(
            builtins.len(),
            crate::config::BuiltinTools::all_tools().len(),
        );
    }

    #[test]
    fn builtins_enabled_tools_filters() {
        let config = crate::config::AgentConfig {
            capabilities: Some(crate::config::CapabilitiesConfig {
                enabled_tools: Some(vec![
                    crate::config::BuiltinTools::ViewFile,
                    crate::config::BuiltinTools::ListDir,
                ]),
                ..crate::config::CapabilitiesConfig::default()
            }),
            ..crate::config::AgentConfig::default()
        };
        let builtins = super::compute_active_builtins(&config);
        assert_eq!(builtins.len(), 2);
        assert!(builtins.contains(&crate::config::BuiltinTools::ViewFile));
        assert!(builtins.contains(&crate::config::BuiltinTools::ListDir));
    }

    #[test]
    fn builtins_disabled_tools_excludes() {
        let config = crate::config::AgentConfig {
            capabilities: Some(crate::config::CapabilitiesConfig {
                disabled_tools: Some(vec![crate::config::BuiltinTools::RunCommand]),
                ..crate::config::CapabilitiesConfig::default()
            }),
            ..crate::config::AgentConfig::default()
        };
        let builtins = super::compute_active_builtins(&config);
        assert!(
            !builtins.contains(&crate::config::BuiltinTools::RunCommand),
            "RunCommand should be excluded"
        );
        assert!(
            builtins.len() == crate::config::BuiltinTools::all_tools().len() - 1,
            "should have all builtins minus the disabled one"
        );
    }

    #[test]
    fn builtins_custom_tools_only_returns_empty() {
        let config = crate::config::AgentConfig {
            capabilities: Some(crate::config::CapabilitiesConfig::custom_tools_only()),
            ..crate::config::AgentConfig::default()
        };
        let builtins = super::compute_active_builtins(&config);
        assert!(
            builtins.is_empty(),
            "custom_tools_only should produce 0 builtins"
        );
    }

    #[test]
    fn builtins_all_descriptions_non_empty() {
        for tool in crate::config::BuiltinTools::all_tools() {
            assert!(
                !tool.description().is_empty(),
                "builtin {tool:?} has empty description",
            );
        }
    }

    #[test]
    fn builtins_all_sdk_names_non_empty() {
        for tool in crate::config::BuiltinTools::all_tools() {
            assert!(
                !tool.as_sdk_name().is_empty(),
                "builtin {tool:?} has empty SDK name",
            );
        }
    }
}

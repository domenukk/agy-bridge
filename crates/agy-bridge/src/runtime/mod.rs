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

#![expect(clippy::useless_conversion)] // PyO3 #[pyfunction] wrapper generates .into() on PyErr
use std::{collections::HashMap, sync::Arc, time::Duration};

use pyo3::prelude::*;
use tokio::sync::{mpsc, oneshot};

use crate::{error::Error, quota::QuotaState};

pub(crate) mod command_loop;
mod handlers;
pub(crate) mod py_scripts;
pub(crate) mod streaming;
pub(crate) mod venv;

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

/// Opaque agent identifier returned by the runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct AgentId(pub(crate) u64);

impl std::fmt::Display for AgentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "agent-{}", self.0)
    }
}

/// Per-agent state stored in the global [`BRIDGE_STATE`] registry.
///
/// Bundles all sidecar data that FFI callbacks need to look up by agent ID.
/// Consolidating into one struct means a single lock acquisition covers all
/// lookups/insertions/removals, preventing inconsistent partial state.
pub(crate) struct AgentBridgeState {
    /// Custom Rust tools registered for this agent.
    pub(crate) registry: Option<Arc<crate::tools::ToolRegistry>>,
    /// Lifecycle hooks for pre/post turn, tool-call gating, etc.
    pub(crate) hook_runner: Option<Arc<crate::hooks::Hooks>>,
    /// Policy rules governing tool-call permissions.
    pub(crate) policies: crate::policies::PolicySet,
    /// Interactive confirmation handler for `NeedsConfirmation` policies.
    pub(crate) policy_handler: Option<Arc<dyn crate::policies::AskUserHandler>>,
    /// Shared key-value state persisted across tool calls for this agent.
    pub(crate) tool_state: Arc<std::sync::RwLock<HashMap<String, serde_json::Value>>>,
}

/// Single global registry of per-agent bridge state, keyed by agent ID.
///
/// # Lock choice
///
/// Uses `std::sync::RwLock` (not `tokio::sync::RwLock`) because the lock is
/// held only for brief `HashMap` insert/remove/lookup operations and is never
/// held across an `.await` point. This avoids the overhead of an async lock
/// and is safe from deadlocks.
///
/// # Scalability
///
/// For typical agent counts (< ~100), `RwLock<HashMap>` provides sufficient
/// throughput.  Read-side contention is bounded by the microsecond-scale lock
/// duration.  If the bridge ever needs to support thousands of concurrent
/// agents, replacing this with a `DashMap` would eliminate read-lock overhead
/// entirely — but is unnecessary for current workloads.
static BRIDGE_STATE: std::sync::OnceLock<
    std::sync::RwLock<std::collections::HashMap<u64, AgentBridgeState>>,
> = std::sync::OnceLock::new();

/// Access the global per-agent bridge state registry.
pub(crate) fn bridge_state()
-> &'static std::sync::RwLock<std::collections::HashMap<u64, AgentBridgeState>> {
    BRIDGE_STATE.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()))
}

/// Fallback `Hooks` registry used during `create_agent` when the permanent entry is not yet registered.
pub(crate) static INITIALIZING_HOOK_RUNNER: std::sync::Mutex<Option<Arc<crate::hooks::Hooks>>> =
    std::sync::Mutex::new(None);

/// Serializes `create_agent` calls that install a temporary hook runner in
/// [`INITIALIZING_HOOK_RUNNER`], preventing concurrent creates from
/// overwriting each other's fallback runner.
pub(crate) static CREATE_AGENT_HOOK_GUARD: tokio::sync::Mutex<()> =
    tokio::sync::Mutex::const_new(());

/// Execute a hook by name, deserializing the context JSON and calling the
/// appropriate method on the runner. Returns the serialized result (empty
/// string for void hooks).
fn dispatch_hook_by_name(
    hook_runner: &crate::hooks::Hooks,
    hook_point: &str,
    context_json: &str,
) -> Result<String, crate::error::Error> {
    let mut result_json = String::new();
    match hook_point {
        "pre_turn" => {
            let ctx = serde_json::from_str::<crate::hooks::PreTurnContext>(context_json).map_err(
                |e| crate::error::Error::BackendError {
                    message: format!("Failed to deserialize PreTurnContext: {e}"),
                },
            )?;
            hook_runner.run_pre_turn(&ctx);
        }
        "post_turn" => {
            let ctx = serde_json::from_str::<crate::hooks::PostTurnContext>(context_json).map_err(
                |e| crate::error::Error::BackendError {
                    message: format!("Failed to deserialize PostTurnContext: {e}"),
                },
            )?;
            hook_runner.run_post_turn(&ctx);
        }
        "pre_tool_call_decide" => {
            let ctx = serde_json::from_str::<crate::hooks::PreToolCallDecideContext>(context_json)
                .map_err(|e| crate::error::Error::BackendError {
                    message: format!("Failed to deserialize PreToolCallDecideContext: {e} | JSON was: {context_json}"),
                })?;
            let hook_result = hook_runner.run_pre_tool_call_decide(&ctx);
            result_json = serde_json::to_string(&hook_result).map_err(|e| {
                crate::error::Error::BackendError {
                    message: format!("Failed to serialize PreToolCallDecide result: {e}"),
                }
            })?;
        }
        "post_tool_call" => {
            let ctx = serde_json::from_str::<crate::hooks::PostToolCallContext>(context_json)
                .map_err(|e| crate::error::Error::BackendError {
                    message: format!(
                        "Failed to deserialize PostToolCallContext: {e} | JSON was: {context_json}"
                    ),
                })?;
            hook_runner.run_post_tool_call(&ctx);
        }
        "on_compaction" => {
            let ctx = serde_json::from_str::<crate::hooks::OnCompactionContext>(context_json)
                .map_err(|e| crate::error::Error::BackendError {
                    message: format!("Failed to deserialize OnCompactionContext: {e}"),
                })?;
            hook_runner.run_on_compaction(&ctx);
        }
        "on_session_start" => {
            let ctx = serde_json::from_str::<crate::hooks::OnSessionStartContext>(context_json)
                .map_err(|e| crate::error::Error::BackendError {
                    message: format!("Failed to deserialize OnSessionStartContext: {e}"),
                })?;
            hook_runner.run_on_session_start(&ctx);
        }
        "on_session_end" => {
            let ctx = serde_json::from_str::<crate::hooks::OnSessionEndContext>(context_json)
                .map_err(|e| crate::error::Error::BackendError {
                    message: format!("Failed to deserialize OnSessionEndContext: {e}"),
                })?;
            hook_runner.run_on_session_end(&ctx);
        }
        "on_tool_error" => {
            let ctx = serde_json::from_str::<crate::hooks::OnToolErrorContext>(context_json)
                .map_err(|e| crate::error::Error::BackendError {
                    message: format!("Failed to deserialize OnToolErrorContext: {e}"),
                })?;
            hook_runner.run_on_tool_error(&ctx);
        }
        "on_interaction" => {
            let ctx = serde_json::from_str::<crate::hooks::OnInteractionContext>(context_json)
                .map_err(|e| crate::error::Error::BackendError {
                    message: format!("Failed to deserialize OnInteractionContext: {e}"),
                })?;
            let hook_result = hook_runner.run_on_interaction(&ctx);
            result_json = serde_json::to_string(&hook_result).map_err(|e| {
                crate::error::Error::BackendError {
                    message: format!("Failed to serialize OnInteraction result: {e}"),
                }
            })?;
        }
        _ => {
            tracing::warn!("Unknown hook point: {}", hook_point);
        }
    }
    Ok(result_json)
}

/// Dispatches a Rust hook call from the Python thread.
#[pyfunction]
pub(crate) fn dispatch_rust_hook(
    py: Python<'_>,
    agent_id: u64,
    hook_point: String,
    context_json: String,
) -> PyResult<Bound<'_, PyAny>> {
    tracing::debug!(agent_id, hook_point = %hook_point, "dispatch_rust_hook called from Python");
    let hook_runner = {
        let map = bridge_state().read().map_err(|e| {
            pyo3::exceptions::PyRuntimeError::new_err(format!("Failed to read BRIDGE_STATE: {e}"))
        })?;
        if let Some(entry) = map.get(&agent_id) {
            let runner = entry.hook_runner.as_ref().ok_or_else(|| {
                pyo3::exceptions::PyRuntimeError::new_err(format!(
                    "No active Hooks found for agent ID {agent_id}"
                ))
            })?;
            Arc::clone(runner)
        } else {
            let opt = INITIALIZING_HOOK_RUNNER.lock().map_err(|e| {
                pyo3::exceptions::PyRuntimeError::new_err(format!(
                    "Failed to lock INITIALIZING_HOOK_RUNNER: {e}"
                ))
            })?;
            if let Some(ref runner) = *opt {
                Arc::clone(runner)
            } else {
                return Err(pyo3::exceptions::PyRuntimeError::new_err(format!(
                    "No active bridge state or initializing hook runner found for agent ID {agent_id}"
                )));
            }
        }
    };

    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        // SAFETY CONSTRAINT: Hooks dispatched here MUST NOT acquire the Python
        // GIL. The Python thread (which holds the GIL) is blocked waiting for
        // this future to complete via `future_into_py`. Acquiring the GIL from
        // a blocking thread would deadlock.
        let result = tokio::task::spawn_blocking(move || {
            dispatch_hook_by_name(&hook_runner, &hook_point, &context_json)
        })
        .await
        .map_err(|e| {
            pyo3::exceptions::PyRuntimeError::new_err(format!("Hook execution failed: {e}"))
        })?
        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

        Ok(result)
    })
}

#[pyfunction]
pub(crate) fn dispatch_rust_policy_confirm(
    py: Python<'_>,
    agent_id: u64,
    tool_name: String,
    args_json: String,
) -> PyResult<Bound<'_, PyAny>> {
    tracing::info!(agent_id, tool = %tool_name, "dispatch_rust_policy_confirm called from Python");
    let policy_handler = {
        let map = bridge_state().read().map_err(|e| {
            pyo3::exceptions::PyRuntimeError::new_err(format!("Failed to read BRIDGE_STATE: {e}"))
        })?;
        let entry = map.get(&agent_id).ok_or_else(|| {
            pyo3::exceptions::PyRuntimeError::new_err(format!(
                "No active bridge state found for agent ID {agent_id}"
            ))
        })?;
        let handler = entry.policy_handler.as_ref().ok_or_else(|| {
            pyo3::exceptions::PyRuntimeError::new_err(format!(
                "No active AskUserHandler found for agent ID {agent_id}"
            ))
        })?;
        Arc::clone(handler)
    };

    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        // SAFETY CONSTRAINT: Handlers dispatched here MUST NOT acquire the Python
        // GIL. The Python thread is blocked waiting for this future.
        let args_val: serde_json::Value = serde_json::from_str(&args_json).map_err(|e| {
            pyo3::exceptions::PyValueError::new_err(format!(
                "Failed to parse policy args JSON: {e}"
            ))
        })?;
        let result =
            tokio::task::spawn_blocking(move || policy_handler.confirm(&tool_name, &args_val))
                .await
                .map_err(|e| {
                    pyo3::exceptions::PyRuntimeError::new_err(format!(
                        "Policy confirmation panicked: {e}"
                    ))
                })?;

        Ok(result)
    })
}

/// Evaluates policies and registered handlers to check if a tool execution is allowed.
pub(crate) fn check_tool_execution_allowed(
    agent_id: u64,
    name: &str,
    args_json: &str,
) -> Result<bool, crate::error::Error> {
    let map = bridge_state()
        .read()
        .map_err(|e| crate::error::Error::BackendError {
            message: format!("Failed to read BRIDGE_STATE: {e}"),
        })?;

    let Some(state) = map.get(&agent_id) else {
        return Ok(false);
    };

    let (is_allowed, needs_confirm) = match state.policies.evaluate(name) {
        crate::policies::PolicyDecision::Allow => (true, false),
        crate::policies::PolicyDecision::Deny => (false, false),
        crate::policies::PolicyDecision::NeedsConfirmation { .. } => (false, true),
    };

    if is_allowed {
        return Ok(true);
    }

    if needs_confirm && let Some(ref handler) = state.policy_handler {
        let handler = Arc::clone(handler);
        // Drop the lock before calling the handler (it may block).
        drop(map);
        let args_val: serde_json::Value =
            serde_json::from_str(args_json).map_err(|e| crate::error::Error::BackendError {
                message: format!("Failed to parse policy args JSON: {e}"),
            })?;
        return Ok(handler.confirm(name, &args_val));
    }

    Ok(false)
}

/// Dispatches a Rust tool call from the Python thread.
///
/// Called by `AsyncRustProxy.__call__` in the Python SDK. Uses the stored
/// tokio `Handle` to `block_on` the async `ToolRegistry::dispatch`, which
/// is safe because this function runs on the Python thread (not a tokio worker).
#[pyfunction]
fn dispatch_rust_tool<'py>(
    py: Python<'py>,
    agent_id: u64,
    name: String,
    args_json: &str,
) -> PyResult<Bound<'py, PyAny>> {
    tracing::info!(agent_id, tool = %name, "dispatch_rust_tool called from Python (async)");

    // Evaluate policies before tool dispatch
    let is_allowed = check_tool_execution_allowed(agent_id, &name, args_json)
        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

    if !is_allowed {
        return Err(pyo3::exceptions::PyPermissionError::new_err(format!(
            "Tool '{name}' execution blocked by agent policy rules"
        )));
    }

    let (registry, tool_state) = {
        let map = bridge_state().read().map_err(|e| {
            pyo3::exceptions::PyRuntimeError::new_err(format!("Failed to read BRIDGE_STATE: {e}"))
        })?;
        let entry = map.get(&agent_id).ok_or_else(|| {
            pyo3::exceptions::PyRuntimeError::new_err(format!(
                "No active bridge state found for agent ID {agent_id}"
            ))
        })?;
        let registry = entry.registry.as_ref().ok_or_else(|| {
            pyo3::exceptions::PyRuntimeError::new_err(format!(
                "No active ToolRegistry found for agent ID {agent_id}"
            ))
        })?;
        (Arc::clone(registry), Arc::clone(&entry.tool_state))
    };

    let args: serde_json::Value = serde_json::from_str(args_json).map_err(|e| {
        pyo3::exceptions::PyValueError::new_err(format!("Failed to parse tool arguments JSON: {e}"))
    })?;

    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let ctx = crate::tools::ToolContext::with_shared_state(None, tool_state);
        let output = registry
            .dispatch(&name, args, &ctx)
            .await
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
        // Extract the text content for the Python SDK — metadata stays Rust-side.
        Ok(output.into_content())
    })
}

/// Commands sent from Rust to the Python thread.
///
/// Each variant is constructed in `impl Runtime for PythonRuntime` and
/// dispatched in `command_loop::run_async_command_loop`.
pub(crate) enum PyCommand {
    /// Create a new agent with the given configuration dict as JSON.
    CreateAgent {
        config_json: String,
        reply: oneshot::Sender<Result<AgentId, Error>>,
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
    /// Defaults to the value of `AGI_CHAT_TIMEOUT_SECS` (env var), or 600 s.
    pub chat_timeout: Duration,
    /// Delay injected between successive chat commands to prevent burst requests.
    pub inter_agent_delay: Duration,
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
        self.quota_state.wait_for_quota().await;

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
    pyo3::prepare_freethreaded_python();

    // Environment variables are already loaded by load_dotenv() at bridge
    // construction time, before any threads are spawned.

    // Configure sys.path so the venv's site-packages are importable.
    Python::with_gil(|py| {
        if let Err(e) = venv::configure_python_sys_path(py) {
            tracing::warn!("Failed to configure Python sys.path in runtime thread: {e}");
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
    Python::with_gil(|py| {
        let asyncio = py
            .import_bound("asyncio")
            .map_err(|e| Error::BackendError {
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
        let sys = py.import_bound("sys").map_err(|e| Error::BackendError {
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
            let types = py.import_bound("types").map_err(|e| Error::BackendError {
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
        let run_fut =
            pyo3_async_runtimes::tokio::run_until_complete(event_loop.clone(), async move {
                command_loop::run_async_command_loop(cmd_rx, chat_timeout, inter_agent_delay).await
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

impl crate::agent::Runtime for PythonRuntime {
    async fn create_agent(
        &self,
        config: crate::config::AgentConfig,
    ) -> Result<crate::agent::AgentId, Error> {
        // Report all available tools as requested by the user.
        let mut all_tools = config.custom_tool_names();
        if let Some(ref caps) = config.capabilities {
            if let Some(ref builtins) = caps.enabled_tools {
                all_tools.extend(builtins.iter().map(|b| b.as_sdk_name().to_string()));
            } else if caps.disabled_tools.is_none() {
                // Default is all tools
                all_tools.extend(
                    crate::config::capabilities::BuiltinTools::all_tools()
                        .iter()
                        .map(|b| b.as_sdk_name().to_string()),
                );
            }
        } else {
            all_tools.extend(
                crate::config::capabilities::BuiltinTools::all_tools()
                    .iter()
                    .map(|b| b.as_sdk_name().to_string()),
            );
        }
        tracing::info!(
            "Agent starting with {} available tools: {:?}",
            all_tools.len(),
            all_tools
        );

        let config_json = serde_json::to_string(&config).map_err(|e| Error::BackendError {
            message: format!("Failed to serialize AgentConfig: {e}"),
        })?;

        let raw_id = self
            .send_command("create_agent", false, |reply| PyCommand::CreateAgent {
                config_json,
                reply,
            })
            .await?;

        Ok(raw_id.0)
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
    use super::*;

    fn test_config() -> RuntimeConfig {
        RuntimeConfig {
            channel_capacity: 16,
            operation_timeout: Duration::from_secs(10),
            shutdown_timeout: Duration::from_secs(5),
            chat_timeout: Duration::from_mins(1),
            inter_agent_delay: Duration::from_millis(100),
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
    fn safety_error_structural() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let globals = pyo3::types::PyDict::new_bound(py);
            py.run_bound(
                r#"
class StopCandidateException(Exception):
    pass
err = StopCandidateException("dummy")
"#,
                Some(&globals),
                None,
            )
            .unwrap();

            let err_obj = globals.get_item("err").unwrap().unwrap();
            let err = PyErr::from_value_bound(err_obj);

            let mapped = crate::error::classify_py_error(py, &err);

            assert!(
                !matches!(mapped, crate::error::Error::Safety),
                "Failed: matched Error::Safety based purely on the string name StopCandidateException!"
            );
        });
    }

    #[test]
    fn maxtokens_error_structural() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let globals = pyo3::types::PyDict::new_bound(py);
            py.run_bound(
                r#"
class MaxTokensException(Exception):
    pass
err = MaxTokensException("dummy")
"#,
                Some(&globals),
                None,
            )
            .unwrap();

            let err_obj = globals.get_item("err").unwrap().unwrap();
            let err = PyErr::from_value_bound(err_obj);

            let mapped = crate::error::classify_py_error(py, &err);

            assert!(
                !matches!(mapped, crate::error::Error::MaxTokens),
                "Failed: matched Error::MaxTokens based purely on the string name MaxTokensException!"
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
}

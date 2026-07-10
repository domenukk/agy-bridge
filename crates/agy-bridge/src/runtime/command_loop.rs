/// Async command loop and handlers.
use std::time::Duration;

use futures::stream::StreamExt;
use pyo3::prelude::*;
use tokio::{sync::mpsc, time::timeout};

use super::{
    AgentId, PyCommand,
    handlers::{agent, async_ops, chat, query},
};

/// Timeout applied to `handle_send`, `handle_signal_idle`, and `handle_wait_for_wakeup`.
pub(super) const HANDLER_TIMEOUT: Duration = Duration::from_mins(1);

/// Python module name used for Rust ↔ Python global state.
pub(crate) const AGY_BRIDGE_GLOBALS_MODULE: &str = "_agy_bridge_globals";

/// Type alias for the agent registry mapping IDs to their Python context
/// manager and live agent instance objects.
pub(crate) type RegistryInner = std::collections::HashMap<AgentId, (Py<PyAny>, Py<PyAny>)>;
pub(crate) type AgentRegistry = std::sync::Arc<std::sync::Mutex<RegistryInner>>;

/// Look up an agent by ID in the registry, returning cloned Python objects.
///
/// Returns `None` if the agent is not registered or the mutex is poisoned.
///
/// # Poisoned mutex recovery
///
/// The registry mutex is recovered on poison because:
/// - Entries are fully constructed before insertion (no partial writes).
/// - The worst case after a panic is a stale entry for an agent that failed
///   mid-operation — the entry will be cleaned up by `AgentHandle::drop` or
///   the final `cleanup_remaining_agents` sweep.
/// - Panicking here would bring down the entire command loop, killing all
///   agents — disproportionate when only one agent may have failed.
pub(super) fn lookup_agent_instance(
    registry: &AgentRegistry,
    agent_id: AgentId,
) -> Option<(Py<PyAny>, Py<PyAny>)> {
    let lock = registry.lock().unwrap_or_else(|e| {
        tracing::warn!(
            "Agent registry mutex poisoned — recovering (data is safe because entries \
             are always fully formed before insertion): {e}"
        );
        e.into_inner()
    });
    lock.get(&agent_id)
        .map(|(c, a)| Python::attach(|py| (c.clone_ref(py), a.clone_ref(py))))
}

/// Asynchronous command dispatch loop — live SDK mode.
///
/// Receives [`PyCommand`] messages and delegates each to a focused handler
/// function. The registry of live agents is threaded through the handlers.
pub(crate) async fn run_async_command_loop(
    event_loop: Py<PyAny>,
    mut cmd_rx: mpsc::Receiver<PyCommand>,
    chat_timeout: Duration,
    inter_agent_delay: Duration,
) -> PyResult<()> {
    tracing::info!(
        timeout_secs = chat_timeout.as_secs(),
        "Chat round-trip timeout configured"
    );
    let registry: AgentRegistry = std::sync::Arc::new(std::sync::Mutex::new(RegistryInner::new()));
    let mut active_tasks =
        futures::stream::FuturesUnordered::<futures::future::BoxFuture<'static, ()>>::new();

    loop {
        tokio::select! {
            cmd_opt = cmd_rx.recv() => {
                let Some(cmd) = cmd_opt else {
                    break;
                };
                tracing::debug!("Live-SDK command loop: received command");
                if let DispatchResult::Shutdown = dispatch_async_command(
                    cmd,
                    &registry,
                    &event_loop,
                    chat_timeout,
                    inter_agent_delay,
                    &mut active_tasks,
                ).await {
                    break;
                }
            }
            _ = active_tasks.next(), if !active_tasks.is_empty() => {
                // A background task (chat, send, etc.) completed.
            }
        }
    }

    cleanup_remaining_agents(&registry).await;

    Ok(())
}

/// Clean up any agents still in the registry after the command loop exits.
///
/// Calls `__aexit__` on each context manager so Python-side resources
/// (WebSocket connections, localharness processes, file descriptors) are
/// released. Also clears the global tool/hook/policy registries for each agent.
///
/// Recovers from a poisoned mutex — see [`lookup_agent_instance`] for rationale.
async fn cleanup_remaining_agents(registry: &AgentRegistry) {
    let remaining: Vec<_> = registry
        .lock()
        .unwrap_or_else(|e| {
            tracing::warn!("Agent registry mutex poisoned during cleanup — recovering: {e}");
            e.into_inner()
        })
        .drain()
        .collect();
    if !remaining.is_empty() {
        tracing::info!(
            count = remaining.len(),
            "Cleaning up agents remaining in registry after command loop exit"
        );
    }
    for (agent_id, (ctx_py, _instance)) in remaining {
        tracing::debug!(agent_id = ?agent_id, "Calling __aexit__ on leftover agent");
        cleanup_single_agent(agent_id, ctx_py).await;
    }
}

/// Call `__aexit__` on a single agent's context manager and clean up its
/// global registry entries.
async fn cleanup_single_agent(agent_id: AgentId, ctx_py: Py<PyAny>) {
    let aexit_result = Python::attach(|py| {
        let ctx_bound = ctx_py.bind(py);
        let none = py.None();
        let coro = ctx_bound.call_method1("__aexit__", (&none, &none, &none))?;
        Ok::<_, PyErr>(coro.clone().unbind())
    });

    match aexit_result {
        Ok(aexit_coro_py) => {
            let aexit_fut = Python::attach(|py| {
                let coro = aexit_coro_py.into_bound(py);
                pyo3_async_runtimes::tokio::into_future(coro)
            });
            match aexit_fut {
                Ok(fut) => {
                    // Use a short timeout — we're shutting down.
                    match timeout(Duration::from_secs(10), fut).await {
                        Ok(Ok(_)) => {
                            tracing::debug!(agent_id = ?agent_id, "Agent __aexit__ completed");
                        }
                        Ok(Err(e)) => {
                            tracing::warn!(
                                agent_id = ?agent_id,
                                error = %e,
                                "Agent __aexit__ returned error during cleanup"
                            );
                        }
                        Err(_elapsed) => {
                            tracing::warn!(
                                agent_id = ?agent_id,
                                "Agent __aexit__ timed out during cleanup (10s)"
                            );
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        agent_id = ?agent_id,
                        error = %e,
                        "Failed to convert __aexit__ coro to future"
                    );
                }
            }
        }
        Err(e) => {
            tracing::warn!(
                agent_id = ?agent_id,
                error = %e,
                "Failed to call __aexit__ during cleanup"
            );
        }
    }

    // Also clean up the global bridge state for this agent.
    match super::bridge_state().write() {
        Ok(mut map) => {
            map.remove(&agent_id.0);
        }
        Err(e) => {
            tracing::warn!(
                agent_id = agent_id.0,
                error = %e,
                "BRIDGE_STATE RwLock poisoned during cleanup"
            );
        }
    }
}

/// Outcome of dispatching a single command.
enum DispatchResult {
    Continue,
    Shutdown,
}

/// Dispatch synchronous query commands that don't spawn background tasks.
///
/// Returns `Ok(())` if the command was handled. Returns `Err(cmd)` if
/// the command is not a query variant, giving back ownership to the caller.
fn dispatch_query_command(cmd: PyCommand, registry: &AgentRegistry) -> Result<(), PyCommand> {
    match cmd {
        PyCommand::GetHistory { agent_id, reply } => {
            query::handle_get_history(registry, agent_id, reply);
        }
        PyCommand::GetTurnCount { agent_id, reply } => {
            query::handle_get_turn_count(registry, agent_id, reply);
        }
        PyCommand::GetTotalUsage { agent_id, reply } => {
            query::handle_get_total_usage(registry, agent_id, reply);
        }
        PyCommand::GetLastTurnUsage { agent_id, reply } => {
            query::handle_get_last_turn_usage(registry, agent_id, reply);
        }
        PyCommand::GetCompactionIndices { agent_id, reply } => {
            query::handle_get_compaction_indices(registry, agent_id, reply);
        }
        PyCommand::GetLastResponse { agent_id, reply } => {
            query::handle_get_last_response(registry, agent_id, reply);
        }
        PyCommand::IsIdle { agent_id, reply } => {
            query::handle_is_idle(registry, agent_id, reply);
        }
        other => return Err(other),
    }
    Ok(())
}

/// Dispatch a single [`PyCommand`] to the appropriate handler, spawning
/// async work into `active_tasks` where needed.
async fn dispatch_async_command(
    cmd: PyCommand,
    registry: &AgentRegistry,
    event_loop: &Py<PyAny>,
    chat_timeout: Duration,
    inter_agent_delay: Duration,
    active_tasks: &mut futures::stream::FuturesUnordered<futures::future::BoxFuture<'static, ()>>,
) -> DispatchResult {
    // Phase 1: synchronous query commands — no task spawned.
    let cmd = match dispatch_query_command(cmd, registry) {
        Ok(()) => return DispatchResult::Continue,
        Err(cmd) => cmd,
    };

    // Phase 2: agent lifecycle commands (create, shutdown).
    let cmd =
        match dispatch_lifecycle_command(cmd, registry, event_loop, chat_timeout, active_tasks) {
            Ok(()) => return DispatchResult::Continue,
            Err(cmd) => cmd,
        };

    // Phase 3: chat (has its own async dispatch path).
    let cmd = match cmd {
        PyCommand::Chat {
            agent_id,
            prompt,
            reply,
        } => {
            chat::dispatch_chat_command(
                registry,
                agent_id,
                prompt,
                reply,
                chat_timeout,
                active_tasks,
                inter_agent_delay,
            )
            .await;
            return DispatchResult::Continue;
        }
        other => other,
    };

    // Phase 4: async agent operations (cancel, idle, send, etc.).
    let cmd = match dispatch_agent_operation(cmd, registry, active_tasks) {
        Ok(()) => return DispatchResult::Continue,
        Err(cmd) => cmd,
    };

    // Phase 5: global commands.
    match cmd {
        PyCommand::Shutdown => {
            tracing::info!("Shutdown command received, exiting async command loop");
            DispatchResult::Shutdown
        }
        // All other variants are handled by earlier dispatch phases.
        // Using `_` instead of explicit listing so that adding a new
        // PyCommand variant causes a compile error in the earlier
        // dispatch functions (which use exhaustive matches) rather than
        // being silently caught here.
        _ => {
            unreachable!("all variants handled by earlier dispatch phases")
        }
    }
}

/// Push a handler future into `active_tasks`, cloning shared state as needed.
///
/// Eliminates the repeated `registry.clone()` + `Box::pin(async move { … })`
/// boilerplate that every spawned command arm requires.
fn spawn_agent_task(
    active_tasks: &mut futures::stream::FuturesUnordered<futures::future::BoxFuture<'static, ()>>,
    fut: impl std::future::Future<Output = ()> + Send + 'static,
) {
    active_tasks.push(Box::pin(fut));
}

/// Dispatch agent lifecycle commands: create and shutdown.
///
/// Returns `Ok(())` if handled, `Err(cmd)` if not a lifecycle command.
fn dispatch_lifecycle_command(
    cmd: PyCommand,
    registry: &AgentRegistry,
    event_loop: &Py<PyAny>,
    chat_timeout: Duration,
    active_tasks: &mut futures::stream::FuturesUnordered<futures::future::BoxFuture<'static, ()>>,
) -> Result<(), PyCommand> {
    match cmd {
        PyCommand::CreateAgent { config_json, reply } => {
            let registry = registry.clone();
            let event_loop = Python::attach(|py| event_loop.clone_ref(py));
            spawn_agent_task(active_tasks, async move {
                agent::handle_create_agent(registry, event_loop, chat_timeout, config_json, reply)
                    .await;
            });
        }
        PyCommand::ShutdownAgent { agent_id, reply } => {
            let registry = registry.clone();
            spawn_agent_task(active_tasks, async move {
                agent::handle_shutdown_agent(registry, chat_timeout, agent_id, reply).await;
            });
        }
        other => return Err(other),
    }
    Ok(())
}

/// Dispatch async agent operations: cancel, idle, send, signal, wakeup,
/// clear history, remove last turn, delete, disconnect.
///
/// Returns `Ok(())` if handled, `Err(cmd)` if not an agent operation.
fn dispatch_agent_operation(
    cmd: PyCommand,
    registry: &AgentRegistry,
    active_tasks: &mut futures::stream::FuturesUnordered<futures::future::BoxFuture<'static, ()>>,
) -> Result<(), PyCommand> {
    match cmd {
        PyCommand::Cancel { agent_id, reply } => {
            let registry = registry.clone();
            spawn_agent_task(active_tasks, async move {
                async_ops::handle_cancel(registry, agent_id, reply).await;
            });
        }
        PyCommand::WaitForIdle { agent_id, reply } => {
            let registry = registry.clone();
            spawn_agent_task(active_tasks, async move {
                async_ops::handle_wait_for_idle(registry, agent_id, reply).await;
            });
        }
        PyCommand::ClearHistory { agent_id, reply } => {
            async_ops::handle_clear_history(registry, agent_id, reply);
        }
        PyCommand::RemoveLastTurn { agent_id, reply } => {
            async_ops::handle_remove_last_turn(registry, agent_id, reply);
        }
        PyCommand::Send {
            agent_id,
            prompt,
            reply,
        } => {
            let registry = registry.clone();
            spawn_agent_task(active_tasks, async move {
                async_ops::handle_send(registry, agent_id, prompt, reply).await;
            });
        }
        PyCommand::SignalIdle { agent_id, reply } => {
            let registry = registry.clone();
            spawn_agent_task(active_tasks, async move {
                async_ops::handle_signal_idle(registry, agent_id, reply).await;
            });
        }
        PyCommand::WaitForWakeup {
            agent_id,
            timeout_secs,
            reply,
        } => {
            let registry = registry.clone();
            spawn_agent_task(active_tasks, async move {
                async_ops::handle_wait_for_wakeup(registry, agent_id, timeout_secs, reply).await;
            });
        }
        PyCommand::Delete { agent_id, reply } => {
            let registry = registry.clone();
            spawn_agent_task(active_tasks, async move {
                async_ops::handle_delete(registry, agent_id, reply).await;
            });
        }
        PyCommand::Disconnect { agent_id, reply } => {
            let registry = registry.clone();
            spawn_agent_task(active_tasks, async move {
                async_ops::handle_disconnect(registry, agent_id, reply).await;
            });
        }
        other => return Err(other),
    }
    Ok(())
}

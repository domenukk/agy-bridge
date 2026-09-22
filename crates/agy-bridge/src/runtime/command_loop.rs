/// Async command loop and handlers.
use std::time::Duration;

use futures::stream::StreamExt;
use pyo3::prelude::*;
use tokio::sync::mpsc;

use super::{
    AgentId, PyCommand,
    handlers::{agent, async_ops, chat, query},
};

/// Python module name used for Rust ↔ Python global state.
pub(crate) const AGY_BRIDGE_GLOBALS_MODULE: &str = "_agy_bridge_globals";

/// Pair of Python objects (context manager, agent instance) stored in the registry.
pub(crate) type RegisteredAgentPair = (std::sync::Arc<Py<PyAny>>, std::sync::Arc<Py<PyAny>>);

/// Type alias for the agent registry mapping IDs to their Python context
/// manager and live agent instance objects.
pub(crate) type RegistryInner = std::collections::HashMap<AgentId, RegisteredAgentPair>;
pub(crate) type AgentRegistry = std::sync::Arc<std::sync::Mutex<RegistryInner>>;

/// Deadline the Python side gives itself to unwind one agent.
///
/// Mirrors `_AEXIT_TIMEOUT_SECS` in `py/agent_init.py`, which
/// `AgentLifecycleController.__aexit__` passes to `asyncio.wait_for`. Keep the
/// two in sync.
const PY_AEXIT_TIMEOUT: Duration = Duration::from_secs(3);

/// Grace period added on top of [`PY_AEXIT_TIMEOUT`] for the Rust-side guard.
///
/// The Rust deadline must be *strictly larger* than the Python one. With equal
/// deadlines the Rust `timeout` frequently wins the race, drops the `__aexit__`
/// future, and the Python-side cleanup never completes — leaving the
/// `localharness` child unreaped.
const AEXIT_GRACE: Duration = Duration::from_secs(2);

/// Rust-side deadline for awaiting a leftover agent's `__aexit__`.
const LEFTOVER_AEXIT_TIMEOUT: Duration = PY_AEXIT_TIMEOUT.saturating_add(AEXIT_GRACE);

/// Weak references to the writers of chat turns that are still streaming.
///
/// The command loop cannot reach into the futures it owns in `active_tasks`, so
/// each chat task registers its writer here when it creates one. Weak refs mean
/// a finished turn drops out on its own (the task owns the only strong ref), and
/// the loop can tell "still streaming" from "already done" by upgrading.
///
/// Used on shutdown to push a [`StreamError`](crate::streaming::StreamError)
/// into every turn that could not be drained in time, so its caller observes a
/// failure instead of a silently truncated response.
pub(in crate::runtime) type ActiveChatWriters =
    std::sync::Arc<std::sync::Mutex<Vec<std::sync::Weak<crate::streaming::ChatResponseWriter>>>>;

/// Lock the in-flight chat registry, recovering from a poisoned mutex.
///
/// Recovery is safe for the same reason as [`lookup_agent_instance`]: entries
/// are plain `Weak` handles pushed after construction, so a panic can never
/// leave the vector half-written, and panicking here would tear down the whole
/// command loop over one failed turn.
fn lock_active_chats(
    active_chats: &ActiveChatWriters,
) -> std::sync::MutexGuard<'_, Vec<std::sync::Weak<crate::streaming::ChatResponseWriter>>> {
    active_chats.lock().unwrap_or_else(|e| {
        tracing::warn!("Active-chat registry mutex poisoned — recovering: {e}");
        e.into_inner()
    })
}

/// Record an in-flight chat turn's writer so shutdown can fail it if needed.
///
/// Also drops entries whose task has already finished, keeping the vector
/// proportional to the number of concurrent turns rather than to the total
/// number of turns the runtime has ever served.
pub(in crate::runtime) fn register_active_chat(
    active_chats: &ActiveChatWriters,
    writer: &std::sync::Arc<crate::streaming::ChatResponseWriter>,
) {
    let mut guard = lock_active_chats(active_chats);
    guard.retain(|weak| weak.strong_count() > 0);
    guard.push(std::sync::Arc::downgrade(writer));
}

/// Push a [`StreamError`](crate::streaming::StreamError) into every chat turn
/// that is still streaming, so its caller sees an error rather than a partial
/// response that looks successful.
///
/// Uses `try_send` because the error channel has capacity 1 and the consumer
/// may not be draining it yet — the first error wins, and a full channel means
/// the turn already failed for another reason.
fn fail_active_chats(active_chats: &ActiveChatWriters, reason: &str) {
    let writers = std::mem::take(&mut *lock_active_chats(active_chats));
    for weak in writers {
        let Some(writer) = weak.upgrade() else {
            continue;
        };
        if let Err(e) = writer
            .error_tx
            .try_send(crate::streaming::StreamError::new(reason))
        {
            tracing::warn!(
                error = %e,
                "Could not deliver shutdown error to an in-flight chat \
                 (error channel full or closed)"
            );
        }
    }
}

/// Look up an agent by ID in the registry, returning cloned Python object Arcs.
///
/// Returns `None` if the agent is not registered or the mutex is poisoned.
/// Does not acquire the Python GIL while holding the registry lock.
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
) -> Option<RegisteredAgentPair> {
    let lock = registry.lock().unwrap_or_else(|e| {
        tracing::warn!(
            "Agent registry mutex poisoned — recovering (data is safe because entries \
             are always fully formed before insertion): {e}"
        );
        e.into_inner()
    });
    lock.get(&agent_id).cloned()
}

/// Asynchronous command dispatch loop — live SDK mode.
///
/// Receives [`PyCommand`] messages and delegates each to a focused handler
/// function. The registry of live agents is threaded through the handlers.
///
/// `shutdown_timeout` is the caller's total teardown budget
/// ([`RuntimeConfig::shutdown_timeout`](super::RuntimeConfig::shutdown_timeout));
/// it is split between draining in-flight work and the final agent sweep.
pub(crate) async fn run_async_command_loop(
    event_loop: Py<PyAny>,
    mut cmd_rx: mpsc::Receiver<PyCommand>,
    inter_agent_delay: Duration,
    stream_limits: super::streaming::StreamLimits,
    shutdown_timeout: Duration,
    startup_tx: std::sync::mpsc::SyncSender<Result<(), crate::error::Error>>,
) -> PyResult<()> {
    let registry: AgentRegistry = std::sync::Arc::new(std::sync::Mutex::new(RegistryInner::new()));
    let event_loop = std::sync::Arc::new(event_loop);
    let mut active_tasks =
        futures::stream::FuturesUnordered::<futures::future::BoxFuture<'static, ()>>::new();
    let rate_limiter = chat::ChatRateLimiter::new(inter_agent_delay);
    let active_chats = ActiveChatWriters::default();

    if let Err(e) = startup_tx.send(Ok(())) {
        tracing::debug!(error = %e, "PythonRuntime startup receiver dropped before loop start");
    }

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
                    &rate_limiter,
                    stream_limits,
                    &mut active_tasks,
                    &active_chats,
                ) {
                    break;
                }
            }
            _ = active_tasks.next(), if !active_tasks.is_empty() => {
                // A background task (chat, send, etc.) completed.
            }
        }
    }

    // Stop accepting new commands: senders now fail fast with `ChannelClosed`
    // instead of queueing work nobody will ever run.
    cmd_rx.close();

    // Give in-flight work half of the teardown budget. The rest is reserved for
    // `cleanup_remaining_agents`, so the whole teardown still fits inside the
    // caller's `PythonRuntime::shutdown` join timeout.
    drain_active_tasks(&mut active_tasks, &active_chats, shutdown_timeout / 2).await;
    // Release the Python objects the (possibly unfinished) tasks hold before
    // calling `__aexit__` on whatever is left.
    drop(active_tasks);

    cleanup_remaining_agents(&registry).await;

    Ok(())
}

/// Let already-running background tasks finish before teardown, under a single
/// bounded deadline.
///
/// Dropping `active_tasks` outright would silently truncate in-flight chats: a
/// dropped [`ChatResponseWriter`](crate::streaming::ChatResponseWriter) closes
/// its channels *cleanly*, so `ChatResponseHandle::text()` returns
/// `Ok(partial_text)` and the caller cannot distinguish a completed turn from a
/// torn-down one. If the deadline expires, every chat still streaming is
/// therefore failed explicitly before the tasks are dropped.
async fn drain_active_tasks(
    active_tasks: &mut futures::stream::FuturesUnordered<futures::future::BoxFuture<'static, ()>>,
    active_chats: &ActiveChatWriters,
    deadline: Duration,
) {
    if active_tasks.is_empty() {
        return;
    }
    tracing::info!(
        pending = active_tasks.len(),
        timeout_ms = deadline.as_millis(),
        "Shutdown: draining in-flight tasks before agent cleanup"
    );
    let drain = async {
        while active_tasks.next().await.is_some() {
            // Keep pulling until every task has completed.
        }
    };
    if tokio::time::timeout(deadline, drain).await.is_err() {
        tracing::warn!(
            pending = active_tasks.len(),
            timeout_ms = deadline.as_millis(),
            "Shutdown drain deadline expired — failing chats that are still streaming"
        );
        fail_active_chats(
            active_chats,
            "runtime shut down while the turn was still streaming",
        );
    }
}

/// Clean up any agents still in the registry after the command loop exits.
///
/// Calls `__aexit__` on each context manager so Python-side resources
/// (WebSocket connections, SDK backend processes, file descriptors) are
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
        if let Err(e) = tokio::time::timeout(
            LEFTOVER_AEXIT_TIMEOUT,
            cleanup_single_agent(agent_id, ctx_py),
        )
        .await
        {
            tracing::warn!(agent_id = ?agent_id, error = %e, "Timed out waiting for leftover agent __aexit__");
        }
    }
}

/// Call `__aexit__` on a single agent's context manager and clean up its
/// global registry entries.
async fn cleanup_single_agent(agent_id: AgentId, ctx_py: std::sync::Arc<Py<PyAny>>) {
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
                Ok(fut) => match fut.await {
                    Ok(_) => {
                        tracing::debug!(agent_id = ?agent_id, "Agent __aexit__ completed");
                    }
                    Err(e) => {
                        tracing::warn!(
                            agent_id = ?agent_id,
                            error = %e,
                            "Agent __aexit__ returned error during cleanup"
                        );
                    }
                },
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
        PyCommand::GetSandboxStatus { agent_id, reply } => {
            query::handle_get_sandbox_status(registry, agent_id, reply);
        }
        PyCommand::GetActiveAgentCount { reply } => {
            query::handle_get_active_agent_count(registry, reply);
        }
        other => return Err(other),
    }
    Ok(())
}

/// Dispatch a single [`PyCommand`] to the appropriate handler, spawning
/// async work into `active_tasks` where needed.
fn dispatch_async_command(
    cmd: PyCommand,
    registry: &AgentRegistry,
    event_loop: &std::sync::Arc<Py<PyAny>>,
    rate_limiter: &chat::ChatRateLimiter,
    stream_limits: super::streaming::StreamLimits,
    active_tasks: &mut futures::stream::FuturesUnordered<futures::future::BoxFuture<'static, ()>>,
    active_chats: &ActiveChatWriters,
) -> DispatchResult {
    // Phase 1: synchronous query commands — no task spawned.
    let cmd = match dispatch_query_command(cmd, registry) {
        Ok(()) => return DispatchResult::Continue,
        Err(cmd) => cmd,
    };

    // Phase 2: agent lifecycle commands (create, shutdown).
    let cmd = match dispatch_lifecycle_command(cmd, registry, event_loop, active_tasks) {
        Ok(()) => return DispatchResult::Continue,
        Err(cmd) => cmd,
    };

    // Phase 3: chat (spawns streaming task into active_tasks).
    let cmd = match cmd {
        PyCommand::Chat {
            agent_id,
            prompt,
            reply,
        } => {
            if let Some(task) = chat::dispatch_chat_command(
                registry,
                agent_id,
                prompt,
                reply,
                rate_limiter.clone(),
                stream_limits,
                std::sync::Arc::clone(active_chats),
            ) {
                active_tasks.push(task);
            }
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
        // Every other variant is handled by an earlier dispatch phase. This is
        // *not* statically guaranteed: each phase dispatcher ends in a
        // `other => return Err(other)` catch-all, so a newly added PyCommand
        // variant compiles fine and lands here at runtime. Panicking would
        // unwind the command loop and kill every live agent over one stray
        // command, so log it loudly and keep serving instead.
        unhandled => {
            tracing::error!(
                "Unhandled PyCommand variant reached the final dispatch phase — dropping it. \
                 This is a bug: add the variant to one of the dispatch phases. The caller \
                 will observe Error::ChannelClosed."
            );
            // Dropping the command drops its reply channel, which the caller
            // surfaces as `Error::ChannelClosed` rather than hanging forever.
            drop(unhandled);
            DispatchResult::Continue
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
    event_loop: &std::sync::Arc<Py<PyAny>>,
    active_tasks: &mut futures::stream::FuturesUnordered<futures::future::BoxFuture<'static, ()>>,
) -> Result<(), PyCommand> {
    match cmd {
        PyCommand::CreateAgent {
            agent_id,
            config_json,
            reply,
        } => {
            let registry = registry.clone();
            let event_loop = std::sync::Arc::clone(event_loop);
            spawn_agent_task(active_tasks, async move {
                agent::handle_create_agent(registry, event_loop, agent_id, config_json, reply)
                    .await;
            });
        }
        PyCommand::ShutdownAgent { agent_id, reply } => {
            let registry = registry.clone();
            spawn_agent_task(active_tasks, async move {
                agent::handle_shutdown_agent(registry, agent_id, reply).await;
            });
        }
        other => return Err(other),
    }
    Ok(())
}

/// Dispatch async agent operations: cancel, idle, send, signal, wakeup,
/// clear history, delete, disconnect.
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

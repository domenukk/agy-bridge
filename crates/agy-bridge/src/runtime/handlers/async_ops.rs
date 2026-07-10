/// Async operation handlers: cancel, wait-for-idle, clear-history, send,
/// signal-idle, and wait-for-wakeup.
use std::time::Duration;

use pyo3::prelude::*;
use tokio::{sync::oneshot, time::timeout};

use super::super::{
    AgentId,
    command_loop::{AgentRegistry, HANDLER_TIMEOUT},
    py_scripts::decode_prompt_py,
};
use crate::error::Error;

/// Lock the agent registry, recovering from mutex poisoning.
fn lock_registry(
    registry: &AgentRegistry,
) -> std::sync::MutexGuard<'_, super::super::command_loop::RegistryInner> {
    registry.lock().unwrap_or_else(|e| {
        tracing::error!("Agent registry mutex was poisoned, recovering: {e}");
        e.into_inner()
    })
}

/// Clone the Python agent references from the registry.
///
/// Returns `None` if the agent is not present; never fails on a poisoned mutex
/// (recovers via [`lock_registry`]).
fn clone_agent_refs(registry: &AgentRegistry, agent_id: AgentId) -> Option<(Py<PyAny>, Py<PyAny>)> {
    let lock = lock_registry(registry);
    lock.get(&agent_id)
        .map(|(c, a)| Python::attach(|py| (c.clone_ref(py), a.clone_ref(py))))
}

/// Generic async-op executor that factors out the shared registry-lookup →
/// build-coro → convert-to-future → await → reply pattern used by every handler in this
/// module.
async fn run_py_async_op<T, F, E>(
    registry: AgentRegistry,
    agent_id: AgentId,
    reply: oneshot::Sender<Result<T, Error>>,
    timeout_duration: Option<Duration>,
    op_label: &str,
    build_coro: F,
    extract: E,
) where
    T: Send + std::fmt::Debug + 'static,
    F: for<'py> FnOnce(Python<'py>, &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>>,
    E: FnOnce(Py<PyAny>) -> T,
{
    // 1. Look up the agent in the registry.
    let Some((_ctx, agent_instance)) = clone_agent_refs(&registry, agent_id) else {
        if reply
            .send(Err(Error::BackendError {
                message: format!("Agent ID {agent_id} not found in registry"),
            }))
            // NOLINT: `.is_err()` in `if` — receiver-dropped is logged below
            .is_err()
        {
            tracing::warn!(
                agent_id = ?agent_id,
                "{} reply receiver dropped (agent not found)",
                op_label,
            );
        }
        return;
    };

    // 2. Build the Python coroutine and convert it to a Rust future.
    let fut = Python::attach(|py| -> Result<_, Error> {
        let agent_bound = agent_instance.bind(py);
        let coro = build_coro(py, agent_bound)?;
        pyo3_async_runtimes::tokio::into_future(coro).map_err(|e| Error::BackendError {
            message: e.to_string(),
        })
    });

    let fut = match fut {
        Ok(fut) => fut,
        Err(e) => {
            if reply.send(Err(e)).is_err() {
                tracing::warn!(
                    agent_id = ?agent_id,
                    "{} reply receiver dropped (coro error)",
                    op_label,
                );
            }
            return;
        }
    };

    // 3. Await with optional timeout.
    let py_result = if let Some(dur) = timeout_duration {
        match timeout(dur, fut).await {
            Ok(result) => result,
            Err(_elapsed) => {
                let err_msg = format!(
                    "handle_{op_label} timed out after {:.1}s for agent {agent_id}",
                    dur.as_secs_f64(),
                );
                tracing::error!(agent_id = ?agent_id, "{err_msg}");
                if reply
                    .send(Err(Error::Timeout {
                        duration: dur,
                        operation: format!("{op_label}(agent={agent_id})"),
                    }))
                    // NOLINT: `.is_err()` in `if` — receiver-dropped is logged below
                    .is_err()
                {
                    tracing::warn!(
                        agent_id = ?agent_id,
                        "{} reply receiver dropped (timeout)",
                        op_label,
                    );
                }
                return;
            }
        }
    } else {
        fut.await
    };

    // 4. Map the Python result and send the reply.
    match py_result {
        Ok(obj) => {
            if reply.send(Ok(extract(obj))).is_err() {
                tracing::warn!(
                    agent_id = ?agent_id,
                    "{} reply receiver dropped",
                    op_label,
                );
            }
        }
        Err(e) => {
            let err: Error = e.into();
            if reply.send(Err(err)).is_err() {
                tracing::warn!(
                    agent_id = ?agent_id,
                    "{} reply receiver dropped (error)",
                    op_label,
                );
            }
        }
    }
}

pub(in crate::runtime) async fn handle_cancel(
    registry: AgentRegistry,
    agent_id: AgentId,
    reply: oneshot::Sender<Result<(), Error>>,
) {
    run_py_async_op(
        registry,
        agent_id,
        reply,
        Some(HANDLER_TIMEOUT),
        "cancel",
        |_, agent| {
            let conv = agent.getattr("conversation")?;
            conv.call_method0("cancel")
        },
        |_py_none| (),
    )
    .await;
}

pub(in crate::runtime) async fn handle_wait_for_idle(
    registry: AgentRegistry,
    agent_id: AgentId,
    reply: oneshot::Sender<Result<(), Error>>,
) {
    run_py_async_op(
        registry,
        agent_id,
        reply,
        Some(HANDLER_TIMEOUT),
        "wait_for_idle",
        |_, agent| {
            let conv = agent.getattr("conversation")?;
            conv.call_method0("wait_for_idle")
        },
        |_py_none| (),
    )
    .await;
}

/// Clear the conversation history synchronously via `PyO3`.
pub(in crate::runtime) fn handle_clear_history(
    registry: &AgentRegistry,
    agent_id: AgentId,
    reply: oneshot::Sender<Result<(), Error>>,
) {
    let Some((_ctx, agent_instance)) = clone_agent_refs(registry, agent_id) else {
        if reply
            .send(Err(Error::BackendError {
                message: format!("Agent ID {agent_id} not found in registry"),
            }))
            // NOLINT: `.is_err()` in `if` — receiver-dropped is logged below
            .is_err()
        {
            tracing::warn!(
                agent_id = ?agent_id,
                "clear_history reply receiver dropped (agent not found)",
            );
        }
        return;
    };

    let result = Python::attach(|py| -> Result<(), Error> {
        let agent_bound = agent_instance.bind(py);
        if agent_bound.hasattr("conversation")? {
            let conv = agent_bound.getattr("conversation")?;
            if !conv.is_none() && conv.hasattr("clear_history")? {
                conv.call_method0("clear_history")?;
            }
        }
        Ok(())
    });

    if reply.send(result).is_err() {
        tracing::warn!(agent_id = ?agent_id, "clear_history reply receiver dropped");
    }
}

/// Remove the last user+model turn pair from conversation history.
///
/// Accesses `conversation._history` (the SDK's internal mutable list)
/// and removes the last 2 entries. This is a synchronous operation because
/// it only mutates an in-memory Python list.
///
/// Used for safety recovery: when a model refuses due to safety filters,
/// removing the refusal from history and retrying gives it a fresh chance.
pub(in crate::runtime) fn handle_remove_last_turn(
    registry: &AgentRegistry,
    agent_id: AgentId,
    reply: oneshot::Sender<Result<(), Error>>,
) {
    let Some((_ctx, agent_instance)) = clone_agent_refs(registry, agent_id) else {
        if reply
            .send(Err(Error::BackendError {
                message: format!("Agent ID {agent_id} not found in registry"),
            }))
            // NOLINT: `.is_err()` in `if` — receiver-dropped is logged below
            .is_err()
        {
            tracing::warn!(
                agent_id = ?agent_id,
                "remove_last_turn reply receiver dropped (agent not found)",
            );
        }
        return;
    };

    let result = Python::attach(|py| -> Result<(), Error> {
        let agent_bound = agent_instance.bind(py);
        if !agent_bound.hasattr("conversation")? {
            return Ok(());
        }
        let conv = agent_bound.getattr("conversation")?;
        if conv.is_none() {
            return Ok(());
        }

        // The SDK stores the mutable history in `_history` (a Python list).
        // We remove the last 2 entries (user message + model response).
        if conv.hasattr("_history")? {
            let history = conv.getattr("_history")?;
            let len: usize = history.len()?;
            if len >= 2 {
                // Remove from the end: index len-1 first, then len-2.
                history.call_method1("pop", (len - 1,))?;
                history.call_method1("pop", (len - 2,))?;
                tracing::debug!(
                    agent_id = ?agent_id,
                    removed = 2,
                    remaining = len - 2,
                    "Removed last turn pair from conversation history",
                );
            } else {
                tracing::warn!(
                    agent_id = ?agent_id,
                    history_len = len,
                    "Cannot remove last turn: history has fewer than 2 entries",
                );
            }
        } else {
            tracing::warn!(
                agent_id = ?agent_id,
                "Conversation object does not have _history attribute",
            );
        }
        Ok(())
    });

    if reply.send(result).is_err() {
        tracing::warn!(agent_id = ?agent_id, "remove_last_turn reply receiver dropped");
    }
}

pub(in crate::runtime) async fn handle_send(
    registry: AgentRegistry,
    agent_id: AgentId,
    prompt: String,
    reply: oneshot::Sender<Result<(), Error>>,
) {
    run_py_async_op(
        registry,
        agent_id,
        reply,
        Some(HANDLER_TIMEOUT),
        "send",
        |py, agent| {
            let conv = agent.getattr("conversation")?;
            let decoded = decode_prompt_py(py, &prompt)?;
            conv.call_method1("send", (decoded,))
        },
        |_py_none| (),
    )
    .await;
}

pub(in crate::runtime) async fn handle_signal_idle(
    registry: AgentRegistry,
    agent_id: AgentId,
    reply: oneshot::Sender<Result<(), Error>>,
) {
    run_py_async_op(
        registry,
        agent_id,
        reply,
        Some(HANDLER_TIMEOUT),
        "signal_idle",
        |_, agent| {
            let conv = agent.getattr("conversation")?;
            conv.call_method0("signal_idle")
        },
        |_py_none| (),
    )
    .await;
}

pub(in crate::runtime) async fn handle_wait_for_wakeup(
    registry: AgentRegistry,
    agent_id: AgentId,
    timeout_secs: f64,
    reply: oneshot::Sender<Result<bool, Error>>,
) {
    // Use Python's own timeout plus 5s headroom so the Rust side doesn't fire first.
    let wakeup_timeout = Duration::from_secs_f64(timeout_secs + 5.0);
    run_py_async_op(
        registry,
        agent_id,
        reply,
        Some(wakeup_timeout),
        "wait_for_wakeup",
        |_, agent| {
            let conv = agent.getattr("conversation")?;
            conv.call_method1("wait_for_wakeup", (timeout_secs,))
        },
        |result| {
            Python::attach(|py| {
                result.bind(py).extract::<bool>().unwrap_or_else(|e| {
                    tracing::error!(
                        "wait_for_wakeup: failed to extract bool from Python result, \
                         defaulting to false (not woken): {e}"
                    );
                    false
                })
            })
        },
    )
    .await;
}

/// Delete the conversation and all associated state via the async Python API.
pub(in crate::runtime) async fn handle_delete(
    registry: AgentRegistry,
    agent_id: AgentId,
    reply: oneshot::Sender<Result<(), Error>>,
) {
    run_py_async_op(
        registry,
        agent_id,
        reply,
        Some(HANDLER_TIMEOUT),
        "delete",
        |_, agent| {
            let conv = agent.getattr("conversation")?;
            conv.call_method0("delete")
        },
        |_py_none| (),
    )
    .await;
}

/// Disconnect from the agent without deleting state via the async Python API.
pub(in crate::runtime) async fn handle_disconnect(
    registry: AgentRegistry,
    agent_id: AgentId,
    reply: oneshot::Sender<Result<(), Error>>,
) {
    run_py_async_op(
        registry,
        agent_id,
        reply,
        Some(HANDLER_TIMEOUT),
        "disconnect",
        |_, agent| {
            let conv = agent.getattr("conversation")?;
            conv.call_method0("disconnect")
        },
        |_py_none| (),
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that `lock_registry` recovers from a poisoned mutex instead of
    /// panicking, and that the guard still provides access to the inner data.
    #[test]
    fn lock_registry_recovers_from_poisoned_mutex() {
        let registry: AgentRegistry =
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));

        // Poison the mutex by panicking inside a lock scope.
        let registry_clone = registry.clone();
        drop(
            std::thread::spawn(move || {
                let _guard = registry_clone.lock().unwrap();
                panic!("intentional panic to poison mutex");
            })
            .join(),
        );

        // The mutex should be poisoned now.
        assert!(registry.lock().is_err(), "Mutex should be poisoned");

        // lock_registry should recover gracefully.
        let guard = lock_registry(&registry);
        assert!(guard.is_empty());
    }

    /// Confirm that `handle_cancel` returns a `BackendError` when the agent is
    /// missing from the registry rather than panicking.
    #[tokio::test]
    async fn handle_cancel_missing_agent_returns_error() {
        let registry: AgentRegistry =
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let (tx, rx) = oneshot::channel();
        let agent_id = AgentId(9999);

        handle_cancel(registry, agent_id, tx).await;

        let result = rx.await.expect("reply channel should not be dropped");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, Error::BackendError { ref message } if message.contains("9999")),
            "Expected BackendError mentioning the agent ID, got: {err:?}"
        );
    }

    /// Confirm that `handle_wait_for_idle` returns a `BackendError` when the
    /// agent is missing from the registry.
    #[tokio::test]
    async fn handle_wait_for_idle_missing_agent_returns_error() {
        let registry: AgentRegistry =
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let (tx, rx) = oneshot::channel();
        let agent_id = AgentId(42);

        handle_wait_for_idle(registry, agent_id, tx).await;

        let result = rx.await.expect("reply channel should not be dropped");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, Error::BackendError { ref message } if message.contains("42")),
            "Expected BackendError mentioning the agent ID, got: {err:?}"
        );
    }

    /// Confirm that `handle_clear_history` returns a `BackendError` when the
    /// agent is missing from the registry.
    #[tokio::test]
    async fn handle_clear_history_missing_agent_returns_error() {
        let registry: AgentRegistry =
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let (tx, rx) = oneshot::channel();
        let agent_id = AgentId(77);

        handle_clear_history(&registry, agent_id, tx);

        let result = rx.await.expect("reply channel should not be dropped");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, Error::BackendError { ref message } if message.contains("77")),
            "Expected BackendError mentioning the agent ID, got: {err:?}"
        );
    }

    /// Confirm that `handle_send` returns a `BackendError` when the agent is
    /// missing from the registry.
    #[tokio::test]
    async fn handle_send_missing_agent_returns_error() {
        let registry: AgentRegistry =
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let (tx, rx) = oneshot::channel();
        let agent_id = AgentId(100);

        handle_send(registry, agent_id, "hello".to_owned(), tx).await;

        let result = rx.await.expect("reply channel should not be dropped");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, Error::BackendError { ref message } if message.contains("100")),
            "Expected BackendError mentioning the agent ID, got: {err:?}"
        );
    }

    /// Confirm that `handle_signal_idle` returns a `BackendError` when the
    /// agent is missing from the registry.
    #[tokio::test]
    async fn handle_signal_idle_missing_agent_returns_error() {
        let registry: AgentRegistry =
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let (tx, rx) = oneshot::channel();
        let agent_id = AgentId(55);

        handle_signal_idle(registry, agent_id, tx).await;

        let result = rx.await.expect("reply channel should not be dropped");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, Error::BackendError { ref message } if message.contains("55")),
            "Expected BackendError mentioning the agent ID, got: {err:?}"
        );
    }

    /// Confirm that `handle_wait_for_wakeup` returns a `BackendError` when the
    /// agent is missing from the registry.
    #[tokio::test]
    async fn handle_wait_for_wakeup_missing_agent_returns_error() {
        let registry: AgentRegistry =
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let (tx, rx) = oneshot::channel();
        let agent_id = AgentId(88);

        handle_wait_for_wakeup(registry, agent_id, 1.0, tx).await;

        let result = rx.await.expect("reply channel should not be dropped");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, Error::BackendError { ref message } if message.contains("88")),
            "Expected BackendError mentioning the agent ID, got: {err:?}"
        );
    }

    /// `lock_registry` on a non-poisoned mutex works normally.
    #[test]
    fn lock_registry_normal_operation() {
        let registry: AgentRegistry =
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));

        let guard = lock_registry(&registry);
        assert!(guard.is_empty());
        drop(guard);

        // Can lock again after dropping.
        let guard2 = lock_registry(&registry);
        assert!(guard2.is_empty());
    }

    /// Confirm that `handle_delete` returns a `BackendError` when the agent is
    /// missing from the registry.
    #[tokio::test]
    async fn handle_delete_missing_agent_returns_error() {
        let registry: AgentRegistry =
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let (tx, rx) = oneshot::channel();
        let agent_id = AgentId(200);

        handle_delete(registry, agent_id, tx).await;

        let result = rx.await.expect("reply channel should not be dropped");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, Error::BackendError { ref message } if message.contains("200")),
            "Expected BackendError mentioning the agent ID, got: {err:?}"
        );
    }

    /// Confirm that `handle_disconnect` returns a `BackendError` when the
    /// agent is missing from the registry.
    #[tokio::test]
    async fn handle_disconnect_missing_agent_returns_error() {
        let registry: AgentRegistry =
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let (tx, rx) = oneshot::channel();
        let agent_id = AgentId(300);

        handle_disconnect(registry, agent_id, tx).await;

        let result = rx.await.expect("reply channel should not be dropped");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, Error::BackendError { ref message } if message.contains("300")),
            "Expected BackendError mentioning the agent ID, got: {err:?}"
        );
    }
}

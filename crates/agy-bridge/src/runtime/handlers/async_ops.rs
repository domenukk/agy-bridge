/// Async operation handlers: cancel, wait-for-idle, clear-history, send,
/// signal-idle, and wait-for-wakeup.
use std::time::Duration;

use pyo3::prelude::*;
use tokio::{sync::oneshot, time::timeout};

use super::super::{
    AgentId,
    command_loop::{
        AgentRegistry, CANCEL_FN, CANCEL_FN_NAME, CLEAR_HISTORY_FN, CLEAR_HISTORY_FN_NAME,
        DELETE_FN, DELETE_FN_NAME, DISCONNECT_FN, DISCONNECT_FN_NAME, HANDLER_TIMEOUT, SEND_FN,
        SEND_FN_NAME, SIGNAL_IDLE_FN, SIGNAL_IDLE_FN_NAME, WAIT_FOR_IDLE_FN, WAIT_FOR_IDLE_FN_NAME,
        WAIT_FOR_WAKEUP_FN, WAIT_FOR_WAKEUP_FN_NAME, get_or_compile_py_helper,
    },
    py_scripts::{
        PYTHON_CANCEL_SCRIPT, PYTHON_CLEAR_HISTORY_SCRIPT, PYTHON_DELETE_SCRIPT,
        PYTHON_DISCONNECT_SCRIPT, PYTHON_SEND_SCRIPT, PYTHON_SIGNAL_IDLE_SCRIPT,
        PYTHON_WAIT_FOR_IDLE_SCRIPT, PYTHON_WAIT_FOR_WAKEUP_SCRIPT,
    },
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

/// Describes a Python async operation to run against an agent instance.
struct PyAsyncOp<'a> {
    /// Compiled-function cache slot.
    cache: &'static std::sync::OnceLock<Py<PyAny>>,
    /// Python source that defines the helper function.
    script: &'a str,
    /// Name of the function inside the script.
    fn_name: &'a str,
    /// Rust-side timeout applied to the awaited future.
    /// `None` means no timeout (just `.await` directly).
    timeout: Option<Duration>,
    /// Human-readable operation name for error messages (e.g. `"cancel"`).
    op_label: &'a str,
}

/// Generic async-op executor that factors out the shared registry-lookup →
/// compile → call → await → reply pattern used by every handler in this
/// module.
///
/// `build_args` receives the bound helper function and the bound agent
/// instance and must return the Python arguments tuple for the helper call.
///
/// `extract` converts the successful Python return value into the Rust
/// reply type `T`.
async fn run_py_async_op<T, F, E>(
    registry: AgentRegistry,
    agent_id: AgentId,
    reply: oneshot::Sender<Result<T, Error>>,
    op: PyAsyncOp<'_>,
    build_args: F,
    extract: E,
) where
    T: Send + std::fmt::Debug + 'static,
    F: FnOnce(&Bound<'_, PyAny>, &Bound<'_, PyAny>) -> PyResult<Py<PyAny>>,
    E: FnOnce(Py<PyAny>) -> T,
{
    // 1. Look up the agent in the registry.
    let Some((_ctx, agent_instance)) = clone_agent_refs(&registry, agent_id) else {
        if reply
            .send(Err(Error::BackendError {
                message: format!("Agent ID {agent_id} not found in registry"),
            }))
            .is_err()
        {
            tracing::warn!(
                agent_id = ?agent_id,
                "{} reply receiver dropped (agent not found)",
                op.op_label,
            );
        }
        return;
    };

    // 2. Compile the Python helper and create a future from the coroutine.
    let fut = get_or_compile_py_helper(op.cache, op.script, op.fn_name).and_then(|helper_fn| {
        Python::attach(|py| {
            let helper_bound = helper_fn.bind(py);
            let agent_bound = agent_instance.bind(py);
            let coro = build_args(helper_bound, agent_bound).map_err(|e| format!("{e}"))?;
            pyo3_async_runtimes::tokio::into_future(coro.into_bound(py)).map_err(|e| format!("{e}"))
        })
    });

    let fut = match fut {
        Ok(fut) => fut,
        Err(err_msg) => {
            if reply
                .send(Err(Error::BackendError { message: err_msg }))
                .is_err()
            {
                tracing::warn!(
                    agent_id = ?agent_id,
                    "{} reply receiver dropped (coro error)",
                    op.op_label,
                );
            }
            return;
        }
    };

    // 3. Await with optional timeout.
    let py_result = if let Some(dur) = op.timeout {
        match timeout(dur, fut).await {
            Ok(result) => result,
            Err(_elapsed) => {
                let err_msg = format!(
                    "handle_{} timed out after {:.1}s for agent {agent_id}",
                    op.op_label,
                    dur.as_secs_f64(),
                );
                tracing::error!(agent_id = ?agent_id, "{err_msg}");
                if reply
                    .send(Err(Error::Timeout {
                        duration: dur,
                        operation: format!("{}(agent={agent_id})", op.op_label),
                    }))
                    .is_err()
                {
                    tracing::warn!(
                        agent_id = ?agent_id,
                        "{} reply receiver dropped (timeout)",
                        op.op_label,
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
                    op.op_label,
                );
            }
        }
        Err(e) => {
            let err: Error = e.into();
            if reply.send(Err(err)).is_err() {
                tracing::warn!(
                    agent_id = ?agent_id,
                    "{} reply receiver dropped (error)",
                    op.op_label,
                );
            }
        }
    }
}

/// Helper to call a Python helper with just the agent instance (no extra args).
fn call_agent_only(helper: &Bound<'_, PyAny>, agent: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
    helper.call1((agent,)).map(Bound::unbind)
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
        PyAsyncOp {
            cache: &CANCEL_FN,
            script: PYTHON_CANCEL_SCRIPT,
            fn_name: CANCEL_FN_NAME,
            timeout: Some(HANDLER_TIMEOUT),
            op_label: "cancel",
        },
        call_agent_only,
        |_| (),
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
        PyAsyncOp {
            cache: &WAIT_FOR_IDLE_FN,
            script: PYTHON_WAIT_FOR_IDLE_SCRIPT,
            fn_name: WAIT_FOR_IDLE_FN_NAME,
            timeout: Some(HANDLER_TIMEOUT),
            op_label: "wait_for_idle",
        },
        call_agent_only,
        |_| (),
    )
    .await;
}

/// Clear the conversation history via the async Python API.
pub(in crate::runtime) async fn handle_clear_history(
    registry: AgentRegistry,
    agent_id: AgentId,
    reply: oneshot::Sender<Result<(), Error>>,
) {
    run_py_async_op(
        registry,
        agent_id,
        reply,
        PyAsyncOp {
            cache: &CLEAR_HISTORY_FN,
            script: PYTHON_CLEAR_HISTORY_SCRIPT,
            fn_name: CLEAR_HISTORY_FN_NAME,
            timeout: None,
            op_label: "clear_history",
        },
        call_agent_only,
        |_| (),
    )
    .await;
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
        PyAsyncOp {
            cache: &SEND_FN,
            script: &PYTHON_SEND_SCRIPT,
            fn_name: SEND_FN_NAME,
            timeout: Some(HANDLER_TIMEOUT),
            op_label: "send",
        },
        |helper, agent| helper.call1((agent, &prompt)).map(Bound::unbind),
        |_| (),
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
        PyAsyncOp {
            cache: &SIGNAL_IDLE_FN,
            script: PYTHON_SIGNAL_IDLE_SCRIPT,
            fn_name: SIGNAL_IDLE_FN_NAME,
            timeout: Some(HANDLER_TIMEOUT),
            op_label: "signal_idle",
        },
        call_agent_only,
        |_| (),
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
        PyAsyncOp {
            cache: &WAIT_FOR_WAKEUP_FN,
            script: PYTHON_WAIT_FOR_WAKEUP_SCRIPT,
            fn_name: WAIT_FOR_WAKEUP_FN_NAME,
            timeout: Some(wakeup_timeout),
            op_label: "wait_for_wakeup",
        },
        |helper, agent| helper.call1((agent, timeout_secs)).map(Bound::unbind),
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
        PyAsyncOp {
            cache: &DELETE_FN,
            script: PYTHON_DELETE_SCRIPT,
            fn_name: DELETE_FN_NAME,
            timeout: Some(HANDLER_TIMEOUT),
            op_label: "delete",
        },
        call_agent_only,
        |_| (),
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
        PyAsyncOp {
            cache: &DISCONNECT_FN,
            script: PYTHON_DISCONNECT_SCRIPT,
            fn_name: DISCONNECT_FN_NAME,
            timeout: Some(HANDLER_TIMEOUT),
            op_label: "disconnect",
        },
        call_agent_only,
        |_| (),
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

        handle_clear_history(registry, agent_id, tx).await;

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

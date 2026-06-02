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
) -> std::sync::MutexGuard<
    '_,
    std::collections::HashMap<super::super::AgentId, (pyo3::PyObject, pyo3::PyObject)>,
> {
    registry.lock().unwrap_or_else(|e| {
        tracing::error!("Agent registry mutex was poisoned, recovering: {e}");
        e.into_inner()
    })
}

pub(in crate::runtime) async fn handle_cancel(
    registry: AgentRegistry,
    agent_id: AgentId,
    reply: oneshot::Sender<Result<(), Error>>,
) {
    let instance_opt = {
        {
            let lock = lock_registry(&registry);
            if let Some((c, a)) = lock.get(&agent_id) {
                Some(Python::with_gil(|py| (c.clone_ref(py), a.clone_ref(py))))
            } else {
                None
            }
        }
    };
    let Some((_ctx, agent_instance)) = instance_opt else {
        if let Err(e) = reply.send(Err(Error::BackendError {
            message: format!("Agent ID {agent_id} not found in registry"),
        })) {
            tracing::warn!(agent_id = ?agent_id, error = ?e, "Cancel reply receiver dropped (agent not found)");
        }
        return;
    };

    let cancel_helper = get_or_compile_py_helper(&CANCEL_FN, PYTHON_CANCEL_SCRIPT, CANCEL_FN_NAME);
    let cancel_fut = cancel_helper.and_then(|helper_fn| {
        Python::with_gil(|py| {
            let helper_bound = helper_fn.bind(py);
            let agent_bound = agent_instance.bind(py);
            let coro = helper_bound
                .call1((agent_bound,))
                .map_err(|e| format!("{e}"))?;
            pyo3_async_runtimes::tokio::into_future(coro).map_err(|e| format!("{e}"))
        })
    });

    let cancel_fut = match cancel_fut {
        Ok(fut) => fut,
        Err(err_msg) => {
            if let Err(e) = reply.send(Err(Error::BackendError { message: err_msg })) {
                tracing::warn!(agent_id = ?agent_id, error = ?e, "Cancel reply receiver dropped (cancel coro error)");
            }
            return;
        }
    };

    let cancel_result = match timeout(HANDLER_TIMEOUT, cancel_fut).await {
        Ok(result) => result,
        Err(_elapsed) => {
            let err_msg = format!(
                "handle_cancel timed out after {}s for agent {agent_id}",
                HANDLER_TIMEOUT.as_secs()
            );
            tracing::error!(agent_id = ?agent_id, "{err_msg}");
            if let Err(e) = reply.send(Err(Error::Timeout {
                duration: HANDLER_TIMEOUT,
                operation: format!("cancel(agent={agent_id})"),
            })) {
                tracing::warn!(agent_id = ?agent_id, error = ?e, "Cancel reply receiver dropped (timeout)");
            }
            return;
        }
    };
    match cancel_result {
        Ok(_) => {
            if let Err(e) = reply.send(Ok(())) {
                tracing::warn!(agent_id = ?agent_id, error = ?e, "Cancel reply receiver dropped");
            }
        }
        Err(e) => {
            let err: Error = e.into();
            if let Err(e) = reply.send(Err(err)) {
                tracing::warn!(agent_id = ?agent_id, error = ?e, "Cancel reply receiver dropped (cancel error)");
            }
        }
    }
}

pub(in crate::runtime) async fn handle_wait_for_idle(
    registry: AgentRegistry,
    agent_id: AgentId,
    reply: oneshot::Sender<Result<(), Error>>,
) {
    let instance_opt = {
        {
            let lock = lock_registry(&registry);
            if let Some((c, a)) = lock.get(&agent_id) {
                Some(Python::with_gil(|py| (c.clone_ref(py), a.clone_ref(py))))
            } else {
                None
            }
        }
    };
    let Some((_ctx, agent_instance)) = instance_opt else {
        if let Err(e) = reply.send(Err(Error::BackendError {
            message: format!("Agent ID {agent_id} not found in registry"),
        })) {
            tracing::warn!(agent_id = ?agent_id, error = ?e, "WaitForIdle reply receiver dropped (agent not found)");
        }
        return;
    };

    let idle_helper = get_or_compile_py_helper(
        &WAIT_FOR_IDLE_FN,
        PYTHON_WAIT_FOR_IDLE_SCRIPT,
        WAIT_FOR_IDLE_FN_NAME,
    );
    let wait_fut = idle_helper.and_then(|helper_fn| {
        Python::with_gil(|py| {
            let helper_bound = helper_fn.bind(py);
            let agent_bound = agent_instance.bind(py);
            let coro = helper_bound
                .call1((agent_bound,))
                .map_err(|e| format!("{e}"))?;
            pyo3_async_runtimes::tokio::into_future(coro).map_err(|e| format!("{e}"))
        })
    });

    let wait_fut = match wait_fut {
        Ok(fut) => fut,
        Err(err_msg) => {
            if let Err(e) = reply.send(Err(Error::BackendError { message: err_msg })) {
                tracing::warn!(agent_id = ?agent_id, error = ?e, "WaitForIdle reply receiver dropped (coro error)");
            }
            return;
        }
    };

    let wait_result = match timeout(HANDLER_TIMEOUT, wait_fut).await {
        Ok(result) => result,
        Err(_elapsed) => {
            let err_msg = format!(
                "handle_wait_for_idle timed out after {}s for agent {agent_id}",
                HANDLER_TIMEOUT.as_secs()
            );
            tracing::error!(agent_id = ?agent_id, "{err_msg}");
            if let Err(e) = reply.send(Err(Error::Timeout {
                duration: HANDLER_TIMEOUT,
                operation: format!("wait_for_idle(agent={agent_id})"),
            })) {
                tracing::warn!(agent_id = ?agent_id, error = ?e, "WaitForIdle reply receiver dropped (timeout)");
            }
            return;
        }
    };
    match wait_result {
        Ok(_) => {
            if let Err(e) = reply.send(Ok(())) {
                tracing::warn!(agent_id = ?agent_id, error = ?e, "WaitForIdle reply receiver dropped");
            }
        }
        Err(e) => {
            let err: Error = e.into();
            if let Err(e) = reply.send(Err(err)) {
                tracing::warn!(agent_id = ?agent_id, error = ?e, "WaitForIdle reply receiver dropped (error)");
            }
        }
    }
}

/// Clear the conversation history via the async Python API.
pub(in crate::runtime) async fn handle_clear_history(
    registry: AgentRegistry,
    agent_id: AgentId,
    reply: oneshot::Sender<Result<(), Error>>,
) {
    let instance_opt = {
        {
            let lock = lock_registry(&registry);
            if let Some((c, a)) = lock.get(&agent_id) {
                Some(Python::with_gil(|py| (c.clone_ref(py), a.clone_ref(py))))
            } else {
                None
            }
        }
    };
    let Some((_ctx, agent_instance)) = instance_opt else {
        if let Err(e) = reply.send(Err(Error::BackendError {
            message: format!("Agent ID {agent_id} not found in registry"),
        })) {
            tracing::warn!(agent_id = ?agent_id, error = ?e, "ClearHistory reply receiver dropped (not found)");
        }
        return;
    };

    let clear_helper = get_or_compile_py_helper(
        &CLEAR_HISTORY_FN,
        PYTHON_CLEAR_HISTORY_SCRIPT,
        CLEAR_HISTORY_FN_NAME,
    );
    let clear_fut = clear_helper.and_then(|helper_fn| {
        Python::with_gil(|py| {
            let helper_bound = helper_fn.bind(py);
            let agent_bound = agent_instance.bind(py);
            let coro = helper_bound
                .call1((agent_bound,))
                .map_err(|e| format!("{e}"))?;
            pyo3_async_runtimes::tokio::into_future(coro).map_err(|e| format!("{e}"))
        })
    });

    let clear_fut = match clear_fut {
        Ok(fut) => fut,
        Err(err_msg) => {
            if let Err(e) = reply.send(Err(Error::BackendError { message: err_msg })) {
                tracing::warn!(agent_id = ?agent_id, error = ?e, "ClearHistory reply receiver dropped (coro error)");
            }
            return;
        }
    };

    match clear_fut.await {
        Ok(_) => {
            if let Err(e) = reply.send(Ok(())) {
                tracing::warn!(agent_id = ?agent_id, error = ?e, "ClearHistory reply receiver dropped");
            }
        }
        Err(e) => {
            let err: Error = e.into();
            if let Err(e) = reply.send(Err(err)) {
                tracing::warn!(agent_id = ?agent_id, error = ?e, "ClearHistory reply receiver dropped (error)");
            }
        }
    }
}

pub(in crate::runtime) async fn handle_send(
    registry: AgentRegistry,
    agent_id: AgentId,
    prompt: String,
    reply: oneshot::Sender<Result<(), Error>>,
) {
    let instance_opt = {
        {
            let lock = lock_registry(&registry);
            if let Some((c, a)) = lock.get(&agent_id) {
                Some(Python::with_gil(|py| (c.clone_ref(py), a.clone_ref(py))))
            } else {
                None
            }
        }
    };
    let Some((_ctx, agent_instance)) = instance_opt else {
        if let Err(e) = reply.send(Err(Error::BackendError {
            message: format!("Agent ID {agent_id} not found in registry"),
        })) {
            tracing::warn!(agent_id = ?agent_id, error = ?e, "Send reply receiver dropped (agent not found)");
        }
        return;
    };

    let send_helper = get_or_compile_py_helper(&SEND_FN, &PYTHON_SEND_SCRIPT, SEND_FN_NAME);
    let send_fut = send_helper.and_then(|helper_fn| {
        Python::with_gil(|py| {
            let helper_bound = helper_fn.bind(py);
            let agent_bound = agent_instance.bind(py);
            let coro = helper_bound
                .call1((agent_bound, &prompt))
                .map_err(|e| format!("{e}"))?;
            pyo3_async_runtimes::tokio::into_future(coro).map_err(|e| format!("{e}"))
        })
    });

    let send_fut = match send_fut {
        Ok(fut) => fut,
        Err(err_msg) => {
            if let Err(e) = reply.send(Err(Error::BackendError { message: err_msg })) {
                tracing::warn!(agent_id = ?agent_id, error = ?e, "Send reply receiver dropped (coro error)");
            }
            return;
        }
    };

    let send_result = match timeout(HANDLER_TIMEOUT, send_fut).await {
        Ok(result) => result,
        Err(_elapsed) => {
            let err_msg = format!(
                "handle_send timed out after {}s for agent {agent_id}",
                HANDLER_TIMEOUT.as_secs()
            );
            tracing::error!(agent_id = ?agent_id, "{err_msg}");
            if let Err(e) = reply.send(Err(Error::Timeout {
                duration: HANDLER_TIMEOUT,
                operation: format!("send(agent={agent_id})"),
            })) {
                tracing::warn!(agent_id = ?agent_id, error = ?e, "Send reply receiver dropped (timeout)");
            }
            return;
        }
    };
    match send_result {
        Ok(_) => {
            if let Err(e) = reply.send(Ok(())) {
                tracing::warn!(agent_id = ?agent_id, error = ?e, "Send reply receiver dropped");
            }
        }
        Err(e) => {
            let err: Error = e.into();
            if let Err(e) = reply.send(Err(err)) {
                tracing::warn!(agent_id = ?agent_id, error = ?e, "Send reply receiver dropped (error)");
            }
        }
    }
}

pub(in crate::runtime) async fn handle_signal_idle(
    registry: AgentRegistry,
    agent_id: AgentId,
    reply: oneshot::Sender<Result<(), Error>>,
) {
    let instance_opt = {
        {
            let lock = lock_registry(&registry);
            if let Some((c, a)) = lock.get(&agent_id) {
                Some(Python::with_gil(|py| (c.clone_ref(py), a.clone_ref(py))))
            } else {
                None
            }
        }
    };
    let Some((_ctx, agent_instance)) = instance_opt else {
        if let Err(e) = reply.send(Err(Error::BackendError {
            message: format!("Agent ID {agent_id} not found in registry"),
        })) {
            tracing::warn!(agent_id = ?agent_id, error = ?e, "SignalIdle reply receiver dropped (agent not found)");
        }
        return;
    };

    let idle_helper = get_or_compile_py_helper(
        &SIGNAL_IDLE_FN,
        PYTHON_SIGNAL_IDLE_SCRIPT,
        SIGNAL_IDLE_FN_NAME,
    );
    let idle_fut = idle_helper.and_then(|helper_fn| {
        Python::with_gil(|py| {
            let helper_bound = helper_fn.bind(py);
            let agent_bound = agent_instance.bind(py);
            let coro = helper_bound
                .call1((agent_bound,))
                .map_err(|e| format!("{e}"))?;
            pyo3_async_runtimes::tokio::into_future(coro).map_err(|e| format!("{e}"))
        })
    });

    let idle_fut = match idle_fut {
        Ok(fut) => fut,
        Err(err_msg) => {
            if let Err(e) = reply.send(Err(Error::BackendError { message: err_msg })) {
                tracing::warn!(agent_id = ?agent_id, error = ?e, "SignalIdle reply receiver dropped (coro error)");
            }
            return;
        }
    };

    let idle_result = match timeout(HANDLER_TIMEOUT, idle_fut).await {
        Ok(result) => result,
        Err(_elapsed) => {
            let err_msg = format!(
                "handle_signal_idle timed out after {}s for agent {agent_id}",
                HANDLER_TIMEOUT.as_secs()
            );
            tracing::error!(agent_id = ?agent_id, "{err_msg}");
            if let Err(e) = reply.send(Err(Error::Timeout {
                duration: HANDLER_TIMEOUT,
                operation: format!("signal_idle(agent={agent_id})"),
            })) {
                tracing::warn!(agent_id = ?agent_id, error = ?e, "SignalIdle reply receiver dropped (timeout)");
            }
            return;
        }
    };
    match idle_result {
        Ok(_) => {
            if let Err(e) = reply.send(Ok(())) {
                tracing::warn!(agent_id = ?agent_id, error = ?e, "SignalIdle reply receiver dropped");
            }
        }
        Err(e) => {
            let err: Error = e.into();
            if let Err(e) = reply.send(Err(err)) {
                tracing::warn!(agent_id = ?agent_id, error = ?e, "SignalIdle reply receiver dropped (error)");
            }
        }
    }
}

pub(in crate::runtime) async fn handle_wait_for_wakeup(
    registry: AgentRegistry,
    agent_id: AgentId,
    timeout_secs: f64,
    reply: oneshot::Sender<Result<bool, Error>>,
) {
    let instance_opt = {
        {
            let lock = lock_registry(&registry);
            if let Some((c, a)) = lock.get(&agent_id) {
                Some(Python::with_gil(|py| (c.clone_ref(py), a.clone_ref(py))))
            } else {
                None
            }
        }
    };
    let Some((_ctx, agent_instance)) = instance_opt else {
        if let Err(e) = reply.send(Err(Error::BackendError {
            message: format!("Agent ID {agent_id} not found in registry"),
        })) {
            tracing::warn!(agent_id = ?agent_id, error = ?e, "WaitForWakeup reply receiver dropped (agent not found)");
        }
        return;
    };

    let wakeup_helper = get_or_compile_py_helper(
        &WAIT_FOR_WAKEUP_FN,
        PYTHON_WAIT_FOR_WAKEUP_SCRIPT,
        WAIT_FOR_WAKEUP_FN_NAME,
    );
    let wakeup_fut = wakeup_helper.and_then(|helper_fn| {
        Python::with_gil(|py| {
            let helper_bound = helper_fn.bind(py);
            let agent_bound = agent_instance.bind(py);
            let coro = helper_bound
                .call1((agent_bound, timeout_secs))
                .map_err(|e| format!("{e}"))?;
            pyo3_async_runtimes::tokio::into_future(coro).map_err(|e| format!("{e}"))
        })
    });

    let wakeup_fut = match wakeup_fut {
        Ok(fut) => fut,
        Err(err_msg) => {
            if let Err(e) = reply.send(Err(Error::BackendError { message: err_msg })) {
                tracing::warn!(agent_id = ?agent_id, error = ?e, "WaitForWakeup reply receiver dropped (coro error)");
            }
            return;
        }
    };

    // Use Python's own timeout plus 5s headroom so the Rust side doesn't fire first.
    let wakeup_timeout = Duration::from_secs_f64(timeout_secs + 5.0);
    let wakeup_result = match timeout(wakeup_timeout, wakeup_fut).await {
        Ok(result) => result,
        Err(_elapsed) => {
            let err_msg = format!(
                "handle_wait_for_wakeup timed out after {:.1}s for agent {agent_id}",
                wakeup_timeout.as_secs_f64()
            );
            tracing::error!(agent_id = ?agent_id, "{err_msg}");
            if let Err(e) = reply.send(Err(Error::Timeout {
                duration: wakeup_timeout,
                operation: format!("wait_for_wakeup(agent={agent_id})"),
            })) {
                tracing::warn!(agent_id = ?agent_id, error = ?e, "WaitForWakeup reply receiver dropped (timeout)");
            }
            return;
        }
    };
    match wakeup_result {
        Ok(result) => {
            let woken = Python::with_gil(|py| {
                result.bind(py).extract::<bool>().unwrap_or_else(|e| {
                    tracing::warn!("Extraction failed: {}", e);
                    false
                })
            });
            if let Err(e) = reply.send(Ok(woken)) {
                tracing::warn!(agent_id = ?agent_id, error = ?e, "WaitForWakeup reply receiver dropped");
            }
        }
        Err(e) => {
            let err: Error = e.into();
            if let Err(e) = reply.send(Err(err)) {
                tracing::warn!(agent_id = ?agent_id, error = ?e, "WaitForWakeup reply receiver dropped (error)");
            }
        }
    }
}

/// Delete the conversation and all associated state via the async Python API.
pub(in crate::runtime) async fn handle_delete(
    registry: AgentRegistry,
    agent_id: AgentId,
    reply: oneshot::Sender<Result<(), Error>>,
) {
    let instance_opt = {
        {
            let lock = lock_registry(&registry);
            if let Some((c, a)) = lock.get(&agent_id) {
                Some(Python::with_gil(|py| (c.clone_ref(py), a.clone_ref(py))))
            } else {
                None
            }
        }
    };
    let Some((_ctx, agent_instance)) = instance_opt else {
        if let Err(e) = reply.send(Err(Error::BackendError {
            message: format!("Agent ID {agent_id} not found in registry"),
        })) {
            tracing::warn!(agent_id = ?agent_id, error = ?e, "Delete reply receiver dropped (not found)");
        }
        return;
    };

    let delete_helper = get_or_compile_py_helper(&DELETE_FN, PYTHON_DELETE_SCRIPT, DELETE_FN_NAME);
    let delete_fut = delete_helper.and_then(|helper_fn| {
        Python::with_gil(|py| {
            let helper_bound = helper_fn.bind(py);
            let agent_bound = agent_instance.bind(py);
            let coro = helper_bound
                .call1((agent_bound,))
                .map_err(|e| format!("{e}"))?;
            pyo3_async_runtimes::tokio::into_future(coro).map_err(|e| format!("{e}"))
        })
    });

    let delete_fut = match delete_fut {
        Ok(fut) => fut,
        Err(err_msg) => {
            if let Err(e) = reply.send(Err(Error::BackendError { message: err_msg })) {
                tracing::warn!(agent_id = ?agent_id, error = ?e, "Delete reply receiver dropped (coro error)");
            }
            return;
        }
    };

    let delete_result = match timeout(HANDLER_TIMEOUT, delete_fut).await {
        Ok(result) => result,
        Err(_elapsed) => {
            let err_msg = format!(
                "handle_delete timed out after {}s for agent {agent_id}",
                HANDLER_TIMEOUT.as_secs()
            );
            tracing::error!(agent_id = ?agent_id, "{err_msg}");
            if let Err(e) = reply.send(Err(Error::Timeout {
                duration: HANDLER_TIMEOUT,
                operation: format!("delete(agent={agent_id})"),
            })) {
                tracing::warn!(agent_id = ?agent_id, error = ?e, "Delete reply receiver dropped (timeout)");
            }
            return;
        }
    };
    match delete_result {
        Ok(_) => {
            if let Err(e) = reply.send(Ok(())) {
                tracing::warn!(agent_id = ?agent_id, error = ?e, "Delete reply receiver dropped");
            }
        }
        Err(e) => {
            let err: Error = e.into();
            if let Err(e) = reply.send(Err(err)) {
                tracing::warn!(agent_id = ?agent_id, error = ?e, "Delete reply receiver dropped (error)");
            }
        }
    }
}

/// Disconnect from the agent without deleting state via the async Python API.
pub(in crate::runtime) async fn handle_disconnect(
    registry: AgentRegistry,
    agent_id: AgentId,
    reply: oneshot::Sender<Result<(), Error>>,
) {
    let instance_opt = {
        {
            let lock = lock_registry(&registry);
            if let Some((c, a)) = lock.get(&agent_id) {
                Some(Python::with_gil(|py| (c.clone_ref(py), a.clone_ref(py))))
            } else {
                None
            }
        }
    };
    let Some((_ctx, agent_instance)) = instance_opt else {
        if let Err(e) = reply.send(Err(Error::BackendError {
            message: format!("Agent ID {agent_id} not found in registry"),
        })) {
            tracing::warn!(agent_id = ?agent_id, error = ?e, "Disconnect reply receiver dropped (not found)");
        }
        return;
    };

    let disconnect_helper =
        get_or_compile_py_helper(&DISCONNECT_FN, PYTHON_DISCONNECT_SCRIPT, DISCONNECT_FN_NAME);
    let disconnect_fut = disconnect_helper.and_then(|helper_fn| {
        Python::with_gil(|py| {
            let helper_bound = helper_fn.bind(py);
            let agent_bound = agent_instance.bind(py);
            let coro = helper_bound
                .call1((agent_bound,))
                .map_err(|e| format!("{e}"))?;
            pyo3_async_runtimes::tokio::into_future(coro).map_err(|e| format!("{e}"))
        })
    });

    let disconnect_fut = match disconnect_fut {
        Ok(fut) => fut,
        Err(err_msg) => {
            if let Err(e) = reply.send(Err(Error::BackendError { message: err_msg })) {
                tracing::warn!(agent_id = ?agent_id, error = ?e, "Disconnect reply receiver dropped (coro error)");
            }
            return;
        }
    };

    let disconnect_result = match timeout(HANDLER_TIMEOUT, disconnect_fut).await {
        Ok(result) => result,
        Err(_elapsed) => {
            let err_msg = format!(
                "handle_disconnect timed out after {}s for agent {agent_id}",
                HANDLER_TIMEOUT.as_secs()
            );
            tracing::error!(agent_id = ?agent_id, "{err_msg}");
            if let Err(e) = reply.send(Err(Error::Timeout {
                duration: HANDLER_TIMEOUT,
                operation: format!("disconnect(agent={agent_id})"),
            })) {
                tracing::warn!(agent_id = ?agent_id, error = ?e, "Disconnect reply receiver dropped (timeout)");
            }
            return;
        }
    };
    match disconnect_result {
        Ok(_) => {
            if let Err(e) = reply.send(Ok(())) {
                tracing::warn!(agent_id = ?agent_id, error = ?e, "Disconnect reply receiver dropped");
            }
        }
        Err(e) => {
            let err: Error = e.into();
            if let Err(e) = reply.send(Err(err)) {
                tracing::warn!(agent_id = ?agent_id, error = ?e, "Disconnect reply receiver dropped (error)");
            }
        }
    }
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

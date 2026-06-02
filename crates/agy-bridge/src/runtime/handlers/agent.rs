/// Agent lifecycle handlers: creation and shutdown.
use std::time::Duration;

use pyo3::prelude::*;
use tokio::{sync::oneshot, time::timeout};

use super::super::{
    AgentId, BridgeContext,
    command_loop::{AGY_BRIDGE_GLOBALS_MODULE, AgentRegistry},
    dispatch_rust_policy_confirm, dispatch_rust_tool,
    py_scripts::PYTHON_AGENT_INIT_SCRIPT,
};
use crate::error::Error;

/// Timeout for `__aexit__` cleanup when `__aenter__` times out.
const AEXIT_CLEANUP_TIMEOUT: Duration = Duration::from_secs(10);

const DISPATCH_RUST_TOOL_ATTR: &str = "dispatch_rust_tool";
const DISPATCH_RUST_HOOK_ATTR: &str = "dispatch_rust_hook";
const DISPATCH_RUST_POLICY_CONFIRM_ATTR: &str = "dispatch_rust_policy_confirm";

/// Prepares the `_agy_bridge_globals` module and registers the Rust tool registry.
fn prepare_agent_globals(py: Python<'_>) -> PyResult<()> {
    let sys = py.import_bound("sys")?;
    let sys_modules = sys.getattr("modules")?;

    let agy_bridge_globals = if sys_modules.contains(AGY_BRIDGE_GLOBALS_MODULE)? {
        sys_modules.get_item(AGY_BRIDGE_GLOBALS_MODULE)?
    } else {
        let types = py.import_bound("types")?;
        let module = types
            .getattr("ModuleType")?
            .call1((AGY_BRIDGE_GLOBALS_MODULE,))?;
        sys_modules.set_item(AGY_BRIDGE_GLOBALS_MODULE, &module)?;
        module
    };

    let globals_module = agy_bridge_globals.downcast::<pyo3::types::PyModule>()?;
    let func = pyo3::wrap_pyfunction_bound!(dispatch_rust_tool, globals_module)?;
    agy_bridge_globals.setattr(DISPATCH_RUST_TOOL_ATTR, func)?;

    let hook_func =
        pyo3::wrap_pyfunction_bound!(crate::runtime::dispatch_rust_hook, globals_module)?;
    agy_bridge_globals.setattr(DISPATCH_RUST_HOOK_ATTR, hook_func)?;

    let confirm_func = pyo3::wrap_pyfunction_bound!(dispatch_rust_policy_confirm, globals_module)?;
    agy_bridge_globals.setattr(DISPATCH_RUST_POLICY_CONFIRM_ATTR, confirm_func)?;

    globals_module.add_class::<crate::policies::PreToolCallDecideHook>()?;
    globals_module.add_class::<BridgeContext>()?;
    Ok(())
}

/// Executes the Python initialization script and creates the context and agent coroutine.
fn init_agent_instance(
    py: Python<'_>,
    config_json: &str,
    agent_id: u64,
    bridge_ctx: BridgeContext,
    event_loop: &PyObject,
) -> PyResult<(PyObject, PyObject)> {
    let globals = pyo3::types::PyDict::new_bound(py);
    py.run_bound(PYTHON_AGENT_INIT_SCRIPT, Some(&globals), None)?;

    let agent_mod = py.import_bound("google.antigravity.agent")?;
    let agent_cls = agent_mod.getattr("Agent")?;
    let init_agent_fn = globals.get_item("init_agent")?.ok_or_else(|| {
        pyo3::exceptions::PyRuntimeError::new_err(
            "init_agent function not found in globals after running PYTHON_AGENT_INIT_SCRIPT",
        )
    })?;

    let bridge_ctx_py = pyo3::Py::new(py, bridge_ctx)?;
    let val = init_agent_fn.call1((
        config_json,
        agent_id,
        agent_cls,
        bridge_ctx_py,
        event_loop.bind(py),
    ))?;
    let agent_ctx = val.get_item(0)?;
    let aenter_coro = val.get_item(1)?;
    let ctx_py = agent_ctx.to_object(py);
    let aenter_coro_py = aenter_coro.to_object(py);
    Ok((ctx_py, aenter_coro_py))
}

/// Best-effort `__aexit__` cleanup when `__aenter__` times out.
///
/// Without this, timed-out `__aenter__` calls leak zombie localharness
/// processes that accumulate until fork exhaustion.
async fn attempt_aexit_cleanup(ctx_py: &Py<PyAny>, cleanup_timeout: Duration) {
    let cleanup_result: Result<(), String> = async {
        let aexit_coro_py = Python::with_gil(|py| {
            let ctx_bound = ctx_py.bind(py);
            let none = py.None();
            let coro = ctx_bound
                .call_method1("__aexit__", (&none, &none, &none))
                .map_err(|e| format!("failed to call __aexit__: {e}"))?;
            Ok::<_, String>(coro.to_object(py))
        })?;
        let aexit_fut = Python::with_gil(|py| {
            let coro = aexit_coro_py.into_bound(py);
            pyo3_async_runtimes::tokio::into_future(coro)
                .map_err(|e| format!("failed to convert __aexit__ coro: {e}"))
        })?;
        match timeout(cleanup_timeout, aexit_fut).await {
            Ok(Ok(_)) => {
                tracing::info!("__aexit__ cleanup succeeded after __aenter__ timeout");
                Ok(())
            }
            Ok(Err(e)) => Err(format!("__aexit__ returned error: {e}")),
            Err(_) => Err("__aexit__ cleanup itself timed out".to_string()),
        }
    }
    .await;
    if let Err(e) = &cleanup_result {
        tracing::error!(error = %e, "__aexit__ cleanup failed — localharness may be leaked");
    }
}

/// Initialize a Python agent via the SDK, run `__aenter__`, and register it.
pub(in crate::runtime) async fn handle_create_agent(
    registry: AgentRegistry,
    chat_timeout: Duration,
    agent_id: AgentId,
    config_json: String,
    bridge_ctx: BridgeContext,
    reply: oneshot::Sender<Result<AgentId, Error>>,
) {
    tracing::info!(
        agent_id = agent_id.0,
        "Live-SDK: CreateAgent command received"
    );

    let init_result = Python::with_gil(|py| {
        prepare_agent_globals(py)?;
        let locals = pyo3_async_runtimes::tokio::get_current_locals(py)?;
        let event_loop = locals.event_loop(py).to_object(py);
        init_agent_instance(py, &config_json, agent_id.0, bridge_ctx, &event_loop)
    });

    let (ctx_py, aenter_coro_py) = match init_result {
        Ok(pair) => pair,
        Err(e) => {
            let err: Error = e.into();
            if let Err(e) = reply.send(Err(err)) {
                tracing::warn!(error = ?e, "CreateAgent reply receiver dropped (config error)");
            }
            return;
        }
    };

    let aenter_fut = match Python::with_gil(|py| {
        let coro = aenter_coro_py.into_bound(py);
        pyo3_async_runtimes::tokio::into_future(coro)
    }) {
        Ok(fut) => fut,
        Err(e) => {
            let err: Error = e.into();
            if let Err(e) = reply.send(Err(err)) {
                tracing::warn!(error = ?e, "CreateAgent reply receiver dropped (aenter conversion error)");
            }
            return;
        }
    };

    tracing::info!("Live-SDK: awaiting __aenter__");
    let enter_result = match timeout(chat_timeout, aenter_fut).await {
        Ok(result) => result,
        Err(_elapsed) => {
            tracing::error!(
                timeout_secs = chat_timeout.as_secs(),
                "CreateAgent __aenter__ timed out"
            );

            tracing::warn!(
                "__aenter__ timed out — attempting __aexit__ cleanup for leaked harness"
            );
            attempt_aexit_cleanup(&ctx_py, AEXIT_CLEANUP_TIMEOUT).await;

            if let Err(e) = reply.send(Err(Error::Timeout {
                duration: chat_timeout,
                operation: "create_agent(__aenter__)".to_string(),
            })) {
                tracing::warn!(error = ?e, "CreateAgent reply receiver dropped (aenter timeout)");
            }
            return;
        }
    };

    tracing::info!("Live-SDK: __aenter__ completed");
    match enter_result {
        Ok(agent_instance_py) => {
            let aid = agent_id;
            match registry.lock() {
                Ok(mut guard) => {
                    guard.insert(aid, (ctx_py, agent_instance_py));
                    if let Err(e) = reply.send(Ok(aid)) {
                        tracing::warn!(error = ?e, "CreateAgent reply receiver dropped");
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "Agent registry mutex poisoned during insert");
                    if let Err(send_err) = reply.send(Err(Error::BackendError {
                        message: "Agent registry mutex poisoned".to_owned(),
                    })) {
                        tracing::warn!(error = ?send_err, "CreateAgent reply receiver dropped (registry poisoned)");
                    }
                }
            }
        }
        Err(e) => {
            let err: Error = e.into();
            if let Err(e) = reply.send(Err(err)) {
                tracing::warn!(error = ?e, "CreateAgent reply receiver dropped (aenter error)");
            }
        }
    }
}

/// Remove the agent from the registry and run its `__aexit__` cleanup.
pub(in crate::runtime) async fn handle_shutdown_agent(
    registry: AgentRegistry,
    chat_timeout: Duration,
    agent_id: AgentId,
    reply: oneshot::Sender<Result<(), Error>>,
) {
    let Some((ctx_py, _instance)) = (match registry.lock() {
        Ok(mut guard) => guard.remove(&agent_id),
        Err(e) => {
            tracing::error!(error = %e, "Agent registry mutex poisoned during shutdown");
            if let Err(send_err) = reply.send(Err(Error::BackendError {
                message: "Agent registry mutex poisoned".to_owned(),
            })) {
                tracing::warn!(error = ?send_err, "ShutdownAgent reply receiver dropped (registry poisoned)");
            }
            return;
        }
    }) else {
        if let Err(e) = reply.send(Err(Error::BackendError {
            message: format!("Agent ID {agent_id} not found in registry for shutdown"),
        })) {
            tracing::warn!(error = ?e, "ShutdownAgent reply receiver dropped (not found)");
        }
        return;
    };

    let aexit_coro_res = Python::with_gil(|py| {
        let ctx_bound = ctx_py.bind(py);
        let none = py.None();
        let coro = ctx_bound.call_method1("__aexit__", (&none, &none, &none))?;
        Ok::<_, PyErr>(coro.to_object(py))
    });

    let aexit_coro_py = match aexit_coro_res {
        Ok(c) => c,
        Err(e) => {
            let err: Error = e.into();
            if let Err(e) = reply.send(Err(err)) {
                tracing::warn!(agent_id = ?agent_id, error = ?e, "ShutdownAgent reply receiver dropped (aexit coro error)");
            }
            return;
        }
    };

    let aexit_fut = match Python::with_gil(|py| {
        let coro = aexit_coro_py.into_bound(py);
        pyo3_async_runtimes::tokio::into_future(coro)
    }) {
        Ok(fut) => fut,
        Err(e) => {
            let err: Error = e.into();
            if let Err(e) = reply.send(Err(err)) {
                tracing::warn!(agent_id = ?agent_id, error = ?e, "ShutdownAgent reply receiver dropped (aexit conversion error)");
            }
            return;
        }
    };

    let exit_res = match timeout(chat_timeout, aexit_fut).await {
        Ok(result) => result,
        Err(_elapsed) => {
            tracing::error!(
                agent_id = ?agent_id,
                timeout_secs = chat_timeout.as_secs(),
                "ShutdownAgent __aexit__ timed out"
            );
            if let Err(e) = reply.send(Err(Error::Timeout {
                duration: chat_timeout,
                operation: format!("shutdown_agent(__aexit__, agent={agent_id})"),
            })) {
                tracing::warn!(error = ?e, "ShutdownAgent reply receiver dropped (aexit timeout)");
            }
            return;
        }
    };

    match exit_res {
        Ok(_) => {
            if let Err(e) = reply.send(Ok(())) {
                tracing::warn!(agent_id = ?agent_id, error = ?e, "ShutdownAgent reply receiver dropped");
            }
        }
        Err(e) => {
            let err: Error = e.into();
            if let Err(e) = reply.send(Err(err)) {
                tracing::warn!(agent_id = ?agent_id, error = ?e, "ShutdownAgent reply receiver dropped (aexit error)");
            }
        }
    }
}

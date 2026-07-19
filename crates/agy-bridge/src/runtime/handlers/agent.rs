/// Agent lifecycle handlers: creation and shutdown.
use std::ffi::CString;

use pyo3::prelude::*;
use tokio::sync::oneshot;

use super::super::{
    AgentId,
    command_loop::{AGY_BRIDGE_GLOBALS_MODULE, AgentRegistry},
    dispatch_rust_policy_confirm, dispatch_rust_tool,
    py_scripts::PYTHON_AGENT_INIT_SCRIPT,
};
use crate::error::Error;

const DISPATCH_RUST_TOOL_ATTR: &str = "dispatch_rust_tool";
const DISPATCH_RUST_HOOK_ATTR: &str = "dispatch_rust_hook";
const DISPATCH_RUST_POLICY_CONFIRM_ATTR: &str = "dispatch_rust_policy_confirm";

/// Prepares the `_agy_bridge_globals` module and registers the Rust tool registry.
fn prepare_agent_globals(py: Python<'_>) -> PyResult<()> {
    let sys = py.import("sys")?;
    let sys_modules = sys.getattr("modules")?;

    let agy_bridge_globals = if sys_modules.contains(AGY_BRIDGE_GLOBALS_MODULE)? {
        sys_modules.get_item(AGY_BRIDGE_GLOBALS_MODULE)?
    } else {
        let types = py.import("types")?;
        let module = types
            .getattr("ModuleType")?
            .call1((AGY_BRIDGE_GLOBALS_MODULE,))?;
        sys_modules.set_item(AGY_BRIDGE_GLOBALS_MODULE, &module)?;
        module
    };

    let globals_module = agy_bridge_globals.cast::<pyo3::types::PyModule>()?;
    let func = pyo3::wrap_pyfunction!(dispatch_rust_tool, globals_module)?;
    agy_bridge_globals.setattr(DISPATCH_RUST_TOOL_ATTR, func)?;

    let hook_func = pyo3::wrap_pyfunction!(crate::runtime::dispatch_rust_hook, globals_module)?;
    agy_bridge_globals.setattr(DISPATCH_RUST_HOOK_ATTR, hook_func)?;

    let confirm_func = pyo3::wrap_pyfunction!(dispatch_rust_policy_confirm, globals_module)?;
    agy_bridge_globals.setattr(DISPATCH_RUST_POLICY_CONFIRM_ATTR, confirm_func)?;

    globals_module.add_class::<crate::policies::PreToolCallDecideHook>()?;
    Ok(())
}

/// Executes the Python initialization script and creates the context and agent coroutine.
fn init_agent_instance(
    py: Python<'_>,
    config_json: &str,
    next_id: u64,
    event_loop: &Py<PyAny>,
) -> PyResult<(Py<PyAny>, Py<PyAny>)> {
    let globals = pyo3::types::PyDict::new(py);
    let c_script = CString::new(PYTHON_AGENT_INIT_SCRIPT).map_err(|e| {
        pyo3::exceptions::PyValueError::new_err(format!(
            "Python init script contains null byte: {e}"
        ))
    })?;
    py.run(c_script.as_c_str(), Some(&globals), None)?;

    let agent_mod = crate::runtime::py_scripts::import_serialized(py, "google.antigravity.agent")?;
    let agent_cls = agent_mod.getattr("Agent")?;
    let init_agent_fn = globals.get_item("init_agent")?.ok_or_else(|| {
        pyo3::exceptions::PyRuntimeError::new_err(
            "init_agent function not found in globals after running PYTHON_AGENT_INIT_SCRIPT",
        )
    })?;

    let val = init_agent_fn.call1((config_json, next_id, agent_cls, event_loop.bind(py)))?;
    let agent_ctx = val.get_item(0)?;
    let aenter_coro = val.get_item(1)?;
    let ctx_py = agent_ctx.clone().unbind();
    let aenter_coro_py = aenter_coro.clone().unbind();
    Ok((ctx_py, aenter_coro_py))
}

/// Initialize a Python agent via the SDK, run `__aenter__`, and register it.
pub(in crate::runtime) async fn handle_create_agent(
    registry: AgentRegistry,
    event_loop: Py<PyAny>,
    next_id: u64,
    config_json: String,
    reply: oneshot::Sender<Result<(AgentId, Vec<RawToolInfo>), Error>>,
) {
    tracing::info!(agent_id = next_id, "Live-SDK: CreateAgent command received");

    let init_result = Python::attach(|py| {
        prepare_agent_globals(py)?;
        init_agent_instance(py, &config_json, next_id, &event_loop)
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

    let aenter_fut = match Python::attach(|py| {
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
    let enter_result = aenter_fut.await;

    tracing::info!("Live-SDK: __aenter__ completed");
    match enter_result {
        Ok(agent_instance_py) => {
            let aid = AgentId(next_id);
            let tool_defs = extract_tool_definitions(&agent_instance_py);
            match registry.lock() {
                Ok(mut guard) => {
                    guard.insert(aid, (ctx_py, agent_instance_py));
                    if let Err(e) = reply.send(Ok((aid, tool_defs))) {
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

/// Raw tool info extracted from the Python `ToolRunner`.
///
/// This is an intermediate representation — the caller tags each tool with
/// the appropriate [`ToolSource`] and assembles the final [`AvailableTool`] list.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct RawToolInfo {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "serde_json::Value::default")]
    pub parameter_schema: serde_json::Value,
}

/// Extract tool definitions from the Python agent's `_tool_runner.tools`.
///
/// For each tool callable, extracts:
/// - `__name__` → name
/// - `__doc__` → description (may be empty)
/// - `input_schema` → JSON schema (for `ToolWithSchema` subclasses; missing otherwise)
///
/// Falls back to an empty list if extraction fails (non-fatal).
/// Logs warnings for tools that are missing descriptions or schemas.
fn extract_tool_definitions(agent_py: &Py<PyAny>) -> Vec<RawToolInfo> {
    Python::attach(|py| {
        let agent = agent_py.bind(py);
        let tool_runner = match agent.getattr("_tool_runner") {
            Ok(runner) => runner,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Could not access agent._tool_runner — available_tools will be empty"
                );
                return Vec::new();
            }
        };
        let tools_dict = match tool_runner.getattr("tools") {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Could not access _tool_runner.tools — available_tools will be empty"
                );
                return Vec::new();
            }
        };

        // Extract tool info from each callable in the dict by running
        // a small Python helper that reads __name__, __doc__, input_schema.
        let ns = pyo3::types::PyDict::new(py);
        let extract_fn = match py
            .run(
                pyo3::ffi::c_str!(
                    r#"
def _extract(tools_dict):
    import json
    result = []
    for name, fn in tools_dict.items():
        desc = getattr(fn, '__doc__', None) or ''
        schema = getattr(fn, 'input_schema', None) or {}
        result.append({'name': name, 'description': desc, 'parameter_schema': schema})
    return json.dumps(result)
"#
                ),
                None,
                Some(&ns),
            )
            .and_then(|()| {
                ns.get_item("_extract")?.ok_or_else(|| {
                    pyo3::exceptions::PyRuntimeError::new_err(
                        "_extract function not found after running helper script",
                    )
                })
            }) {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Failed to define _extract helper — falling back to names only"
                );
                return extract_tool_names_fallback(agent_py);
            }
        };

        let json_str = match extract_fn.call1((tools_dict,)) {
            Ok(result) => match result.extract::<String>() {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "Failed to extract JSON string from _extract — falling back to names only"
                    );
                    return extract_tool_names_fallback(agent_py);
                }
            },
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Failed to call _extract — falling back to names only"
                );
                return extract_tool_names_fallback(agent_py);
            }
        };

        match serde_json::from_str::<Vec<RawToolInfo>>(&json_str) {
            Ok(infos) => {
                warn_missing_tool_metadata(&infos);
                infos
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Failed to deserialize tool definitions JSON — falling back to names only"
                );
                extract_tool_names_fallback(agent_py)
            }
        }
    })
}

/// Log warnings for tools missing a description or parameter schema.
fn warn_missing_tool_metadata(infos: &[RawToolInfo]) {
    for info in infos {
        if info.description.is_empty() {
            tracing::warn!(
                tool = %info.name,
                "Tool has no description — consider adding a docstring"
            );
        }
        if info.parameter_schema.is_null() || info.parameter_schema == serde_json::json!({}) {
            tracing::warn!(
                tool = %info.name,
                "Tool has no parameter schema"
            );
        }
    }
}

/// Fallback: extract just the tool names (no descriptions/schemas) when
/// the full extraction fails.
fn extract_tool_names_fallback(agent_py: &Py<PyAny>) -> Vec<RawToolInfo> {
    Python::attach(|py| {
        let agent = agent_py.bind(py);
        let names: Vec<String> = match agent
            .getattr("_tool_runner")
            .and_then(|r| r.getattr("tool_names"))
            .and_then(|n| n.extract())
        {
            Ok(names) => names,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Failed to extract tool_names in fallback — returning empty tool list"
                );
                Vec::new()
            }
        };
        names
            .into_iter()
            .map(|name| {
                tracing::warn!(
                    tool = %name,
                    "Falling back to name-only tool info (no description or schema)"
                );
                RawToolInfo {
                    name,
                    description: String::new(),
                    parameter_schema: serde_json::Value::Null,
                }
            })
            .collect()
    })
}

/// Remove the agent from the registry and run its `__aexit__` cleanup.
pub(in crate::runtime) async fn handle_shutdown_agent(
    registry: AgentRegistry,
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

    // Bridge state cleanup is handled by AgentHandle::shutdown() after this
    // handler returns, ensuring hooks dispatched during __aexit__ (e.g.
    // on_session_end) can still find the hook runner.

    let aexit_coro_res = Python::attach(|py| {
        let ctx_bound = ctx_py.bind(py);
        let none = py.None();
        let coro = ctx_bound.call_method1("__aexit__", (&none, &none, &none))?;
        Ok::<_, PyErr>(coro.clone().unbind())
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

    let aexit_fut = match Python::attach(|py| {
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

    let exit_res = aexit_fut.await;

    // Bridge state cleanup handled by AgentHandle::shutdown().

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

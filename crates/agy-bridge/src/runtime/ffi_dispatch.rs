//! FFI dispatch functions for tool, hook, and policy callbacks.
//!
//! These `#[pyfunction]` entries are registered as plain Python callables and
//! receive only the arguments the SDK passes (agent ID + serialized context).
//! They look up per-agent state in [`bridge_state()`] and dispatch to the
//! appropriate Rust handler.

use std::sync::Arc;

use pyo3::prelude::*;

use super::bridge_state::bridge_state;

/// Per-agent hook runners installed *during* `create_agent`, before the
/// permanent [`bridge_state()`] entry exists.
///
/// Hooks such as `on_session_start` can fire from the SDK while the agent's
/// `__aenter__` runs — i.e. before `setup_bridge_state` has registered the
/// permanent entry. This registry provides the hook runner in that window.
///
/// Keyed by (process-globally-unique) agent ID so that concurrent creates —
/// on the same *or* different bridges — never block on a shared lock or
/// clobber each other's runner. Entries are inserted before `create_agent`
/// and removed once the permanent bridge state is registered.
static INITIALIZING_HOOK_RUNNERS: std::sync::OnceLock<
    std::sync::RwLock<std::collections::HashMap<u64, Arc<crate::hooks::Hooks>>>,
> = std::sync::OnceLock::new();

/// Access the per-agent initializing hook-runner registry.
pub(crate) fn initializing_hook_runners()
-> &'static std::sync::RwLock<std::collections::HashMap<u64, Arc<crate::hooks::Hooks>>> {
    INITIALIZING_HOOK_RUNNERS
        .get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()))
}

/// Execute a hook by name, deserializing the context JSON and calling the
/// appropriate method on the runner. Returns the serialized result (empty
/// string for void hooks).
pub(crate) fn dispatch_hook_by_name(
    agent_id: u64,
    hook_runner: &crate::hooks::Hooks,
    hook_point: &str,
    context_json: &str,
) -> Result<String, crate::error::Error> {
    match hook_point {
        "pre_turn" => handle_pre_turn(hook_runner, context_json)?,
        "post_turn" => handle_post_turn(hook_runner, context_json)?,
        "pre_tool_call_decide" => return handle_pre_tool_call_decide(hook_runner, context_json),
        "post_tool_call" => handle_post_tool_call(hook_runner, context_json)?,
        "on_compaction" => handle_on_compaction(hook_runner, context_json)?,
        "on_session_start" => handle_on_session_start(agent_id, hook_runner, context_json)?,
        "on_session_end" => handle_on_session_end(hook_runner, context_json)?,
        "on_tool_error" => handle_on_tool_error(agent_id, hook_runner, context_json)?,
        "on_interaction" => return handle_on_interaction(hook_runner, context_json),
        _ => {
            return Err(crate::error::Error::BackendError {
                message: format!("Unknown hook point: {hook_point}"),
            });
        }
    }
    Ok(String::new())
}

fn deserialize_ctx<'a, T: serde::Deserialize<'a>>(
    context_json: &'a str,
    name: &str,
) -> Result<T, crate::error::Error> {
    serde_json::from_str(context_json).map_err(|e| crate::error::Error::BackendError {
        message: format!("Failed to deserialize {name}: {e} | JSON was: {context_json}"),
    })
}

fn handle_pre_turn(runner: &crate::hooks::Hooks, json: &str) -> Result<(), crate::error::Error> {
    let ctx = deserialize_ctx(json, "PreTurnContext")?;
    runner.run_pre_turn(&ctx);
    Ok(())
}

fn handle_post_turn(runner: &crate::hooks::Hooks, json: &str) -> Result<(), crate::error::Error> {
    let ctx = deserialize_ctx(json, "PostTurnContext")?;
    runner.run_post_turn(&ctx);
    Ok(())
}

fn handle_post_tool_call(
    runner: &crate::hooks::Hooks,
    json: &str,
) -> Result<(), crate::error::Error> {
    let ctx = deserialize_ctx(json, "PostToolCallContext")?;
    runner.run_post_tool_call(&ctx);
    Ok(())
}

fn handle_on_compaction(
    runner: &crate::hooks::Hooks,
    json: &str,
) -> Result<(), crate::error::Error> {
    let ctx = deserialize_ctx(json, "OnCompactionContext")?;
    runner.run_on_compaction(&ctx);
    Ok(())
}

fn handle_on_session_end(
    runner: &crate::hooks::Hooks,
    json: &str,
) -> Result<(), crate::error::Error> {
    let ctx = deserialize_ctx(json, "OnSessionEndContext")?;
    runner.run_on_session_end(&ctx);
    Ok(())
}

fn handle_on_tool_error(
    agent_id: u64,
    runner: &crate::hooks::Hooks,
    json: &str,
) -> Result<(), crate::error::Error> {
    let ctx = deserialize_ctx(json, "OnToolErrorContext")?;
    // Recover the structured error captured during dispatch (see
    // `dispatch_rust_tool`). Taking it clears the slot so it can't leak into a
    // later, unrelated error.
    let captured = super::bridge_state::take_last_tool_error(agent_id);
    let ctx = merge_tool_error_metadata(ctx, captured);
    runner.run_on_tool_error(&ctx);
    Ok(())
}

/// Enrich an [`OnToolErrorContext`](crate::hooks::OnToolErrorContext) with the
/// `metadata` from a captured [`ToolError`](llm_tool::ToolError) value.
///
/// `captured` is the serialized `ToolError` (`{"message": ..., "metadata":
/// {...}}`) recorded on the dispatch error path, or `None` when no structured
/// error was captured (e.g. an error raised on the Python side). When present,
/// its `metadata` object replaces the context's metadata; when absent — or when
/// the captured error carried no metadata — the context's metadata is left as
/// its deserialized default ([`serde_json::Value::Null`]).
///
/// Kept pure (no locks, no globals) so it can be unit-tested directly.
fn merge_tool_error_metadata(
    mut ctx: crate::hooks::OnToolErrorContext,
    captured: Option<serde_json::Value>,
) -> crate::hooks::OnToolErrorContext {
    if let Some(serde_json::Value::Object(mut map)) = captured
        && let Some(metadata) = map.remove("metadata")
    {
        ctx.metadata = metadata;
    }
    ctx
}

fn handle_on_interaction(
    runner: &crate::hooks::Hooks,
    json: &str,
) -> Result<String, crate::error::Error> {
    let ctx = deserialize_ctx(json, "OnInteractionContext")?;
    let hook_result = runner.run_on_interaction(&ctx);
    serde_json::to_string(&hook_result).map_err(|e| crate::error::Error::BackendError {
        message: format!("Failed to serialize OnInteraction result: {e}"),
    })
}

fn handle_pre_tool_call_decide(
    hook_runner: &crate::hooks::Hooks,
    context_json: &str,
) -> Result<String, crate::error::Error> {
    let ctx = serde_json::from_str::<crate::hooks::PreToolCallDecideContext>(context_json)
        .map_err(|e| crate::error::Error::BackendError {
            message: format!(
                "Failed to deserialize PreToolCallDecideContext: {e} | JSON was: {context_json}"
            ),
        })?;
    let transformed_args = hook_runner.run_transform_tool_input(&ctx);
    let hook_result = hook_runner.run_pre_tool_call_decide(&ctx);
    let mut result_val =
        serde_json::to_value(&hook_result).map_err(|e| crate::error::Error::BackendError {
            message: format!("Failed to serialize PreToolCallDecide result: {e}"),
        })?;
    if transformed_args != ctx.tool_args
        && let serde_json::Value::Object(ref mut map) = result_val
    {
        map.insert("transformed_args".to_owned(), transformed_args);
    }
    serde_json::to_string(&result_val).map_err(|e| crate::error::Error::BackendError {
        message: format!("Failed to serialize PreToolCallDecide result: {e}"),
    })
}

fn handle_on_session_start(
    _agent_id: u64,
    hook_runner: &crate::hooks::Hooks,
    context_json: &str,
) -> Result<(), crate::error::Error> {
    let ctx =
        serde_json::from_str::<crate::hooks::OnSessionStartContext>(context_json).map_err(|e| {
            crate::error::Error::BackendError {
                message: format!("Failed to deserialize OnSessionStartContext: {e}"),
            }
        })?;
    // NOTE: Previously this function synced `ctx.session.session_id` into the
    // agent's `conversation_id` field.  That was incorrect — `session_id` is
    // the save-directory basename (e.g. "fixed_run_3"), NOT a real conversation
    // handle.  The conversation_id must be set explicitly by the caller via
    // `AgentHandle::set_conversation_id` or `AgentConfig::conversation_id`.
    hook_runner.run_on_session_start(&ctx);
    Ok(())
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
            let map = initializing_hook_runners().read().map_err(|e| {
                pyo3::exceptions::PyRuntimeError::new_err(format!(
                    "Failed to read initializing hook runners: {e}"
                ))
            })?;
            if let Some(runner) = map.get(&agent_id) {
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
            dispatch_hook_by_name(agent_id, &hook_runner, &hook_point, &context_json)
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
        return Err(crate::error::Error::BackendError {
            message: format!(
                "Agent {agent_id} not found in bridge state — it may have been shut down"
            ),
        });
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

/// Build the [`ToolContext`](crate::tools::ToolContext) for a custom-tool
/// dispatch: the agent's shared, cross-call key-value state plus its current
/// conversation ID (when set), so tools can identify which conversation they
/// are serving via [`ToolContext::conversation_id`](llm_tool::ToolContext::conversation_id).
///
/// The conversation ID is threaded in only when present; an agent that never
/// had one set yields a context whose `conversation_id()` is `None`.
pub(crate) fn build_tool_context(
    tool_state: llm_tool::SharedState,
    conversation_id: Option<String>,
) -> crate::tools::ToolContext {
    let ctx = crate::tools::ToolContext::new().with_shared_state(tool_state);
    match conversation_id {
        Some(id) => ctx.with_conversation_id(id),
        None => ctx,
    }
}

/// Dispatches a Rust tool call from the Python thread.
///
/// Called by `AsyncRustProxy.__call__` in the Python SDK. Uses the stored
/// tokio `Handle` to `block_on` the async `ToolRegistry::dispatch`, which
/// is safe because this function runs on the Python thread (not a tokio worker).
#[pyfunction]
pub(crate) fn dispatch_rust_tool<'py>(
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

    let (registry, tool_state, conversation_id) = {
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
        // Snapshot the agent's current conversation ID (if any) so the tool's
        // `ToolContext` can identify which conversation it serves. A poisoned
        // mutex is logged (never silently swallowed) and treated as "unset"
        // rather than failing the dispatch over missing identity metadata.
        let conversation_id = match entry.conversation_id.lock() {
            Ok(guard) => guard.clone(),
            Err(e) => {
                tracing::error!(
                    agent_id,
                    error = %e,
                    "conversation_id mutex poisoned during tool dispatch — \
                     tool context will omit the conversation ID"
                );
                None
            }
        };
        (
            Arc::clone(registry),
            entry.tool_state.clone(),
            conversation_id,
        )
    };

    let args: serde_json::Value = serde_json::from_str(args_json).map_err(|e| {
        pyo3::exceptions::PyValueError::new_err(format!("Failed to parse tool arguments JSON: {e}"))
    })?;

    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let ctx = build_tool_context(tool_state, conversation_id);
        let output = match registry.dispatch(&name, args, &ctx).await {
            Ok(output) => {
                // Success: drop any stale error so a later `on_tool_error`
                // never surfaces metadata from a previous failed call.
                super::bridge_state::clear_last_tool_error(agent_id);
                output
            }
            Err(e) => {
                // Cache the structured error (message + metadata) *before*
                // collapsing it into a model-facing string, so the
                // `on_tool_error` hook can recover the metadata. The model
                // still sees exactly `ToolError::to_string()`.
                super::bridge_state::record_last_tool_error(agent_id, &e);
                return Err(pyo3::exceptions::PyRuntimeError::new_err(e.to_string()));
            }
        };
        let res = Python::attach(|py| -> PyResult<Py<PyAny>> {
            let dict = pyo3::types::PyDict::new(py);
            dict.set_item("content", output.content())?;
            super::py_scripts::warm_up_lazy_imports(py);
            let metadata_val = pythonize::pythonize(py, output.metadata())?;
            dict.set_item("metadata", metadata_val)?;
            Ok(dict.into_any().unbind())
        })
        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
        Ok(res)
    })
}

#[cfg(test)]
mod merge_tests {
    use super::*;

    fn base_ctx() -> crate::hooks::OnToolErrorContext {
        crate::hooks::OnToolErrorContext {
            tool_name: "add".into(),
            tool_args: serde_json::json!({"a": 1}),
            error: "boom".into(),
            metadata: serde_json::Value::Null,
        }
    }

    #[test]
    fn merge_absent_capture_leaves_metadata_null() {
        let ctx = merge_tool_error_metadata(base_ctx(), None);
        assert_eq!(ctx.metadata, serde_json::Value::Null);
        // Model-facing message is untouched.
        assert_eq!(ctx.error, "boom");
    }

    #[test]
    fn merge_capture_without_metadata_leaves_metadata_null() {
        // A serialized ToolError with no metadata skips the `metadata` field.
        let captured = serde_json::json!({"message": "boom"});
        let ctx = merge_tool_error_metadata(base_ctx(), Some(captured));
        assert_eq!(ctx.metadata, serde_json::Value::Null);
    }

    #[test]
    fn merge_capture_with_metadata_enriches_context() {
        let captured = serde_json::json!({
            "message": "boom",
            "metadata": {"status_code": 503}
        });
        let ctx = merge_tool_error_metadata(base_ctx(), Some(captured));
        assert_eq!(ctx.metadata["status_code"], 503);
        // Other fields are preserved unchanged.
        assert_eq!(ctx.error, "boom");
        assert_eq!(ctx.tool_name, "add");
    }

    #[test]
    fn merge_real_tool_error_roundtrip_surfaces_not_found() {
        // End-to-end shape: serialize a real ToolError exactly as
        // `record_last_tool_error` does, then merge and assert the convenience
        // helper detects it.
        let error = llm_tool::ToolError::not_found(llm_tool::RegistryItem::Tool, "add_nummbers");
        let captured = serde_json::to_value(&error).unwrap();
        let ctx = merge_tool_error_metadata(base_ctx(), Some(captured));
        assert!(ctx.is_not_found());
    }
}

/// Synchronous query handlers that do not spawn background tasks.
use pyo3::prelude::*;
use tokio::sync::oneshot;

use super::super::{
    AgentId,
    command_loop::{AgentRegistry, get_or_compile_py_helper},
    py_scripts::{
        PYTHON_GET_HISTORY_SCRIPT, PYTHON_GET_LAST_TURN_USAGE_SCRIPT,
        PYTHON_GET_TOTAL_USAGE_SCRIPT, PYTHON_GET_TURN_COUNT_SCRIPT, PYTHON_IS_IDLE_SCRIPT,
    },
};
use crate::error::Error;

const GET_HISTORY_FN_NAME: &str = "_get_history";
static GET_HISTORY_FN: std::sync::OnceLock<Py<PyAny>> = std::sync::OnceLock::new();

const GET_TURN_COUNT_FN_NAME: &str = "_get_turn_count";
static GET_TURN_COUNT_FN: std::sync::OnceLock<Py<PyAny>> = std::sync::OnceLock::new();

const GET_TOTAL_USAGE_FN_NAME: &str = "_get_total_usage";
static GET_TOTAL_USAGE_FN: std::sync::OnceLock<Py<PyAny>> = std::sync::OnceLock::new();

const GET_LAST_TURN_USAGE_FN_NAME: &str = "_get_last_turn_usage";
static GET_LAST_TURN_USAGE_FN: std::sync::OnceLock<Py<PyAny>> = std::sync::OnceLock::new();

const IS_IDLE_FN_NAME: &str = "_is_idle";
static IS_IDLE_FN: std::sync::OnceLock<Py<PyAny>> = std::sync::OnceLock::new();

/// A cloned pair of Python references: `(context_manager, agent_instance)`.
type PyAgentRef = (Py<PyAny>, Py<PyAny>);

/// Lock the agent registry and clone the Python references for the given agent.
///
/// Returns `Ok(Some((ctx, instance)))` on success, `Ok(None)` if the agent
/// is not in the registry, or `Err` if the mutex is poisoned.
fn lock_agent_instance(
    registry: &AgentRegistry,
    agent_id: AgentId,
) -> Result<Option<PyAgentRef>, Error> {
    let guard = registry.lock().map_err(|e| {
        tracing::error!(agent_id = ?agent_id, error = %e, "Agent registry mutex poisoned");
        Error::BackendError {
            message: "Agent registry mutex poisoned".to_owned(),
        }
    })?;
    Ok(guard
        .get(&agent_id)
        .map(|(c, a)| Python::attach(|py| (c.clone_ref(py), a.clone_ref(py)))))
}

/// Extract conversation history from the agent's conversation object.
pub(in crate::runtime) fn handle_get_history(
    registry: &AgentRegistry,
    agent_id: AgentId,
    reply: oneshot::Sender<Result<Vec<crate::types::ConversationMessage>, Error>>,
) {
    let instance_opt = match lock_agent_instance(registry, agent_id) {
        Ok(opt) => opt,
        Err(e) => {
            if let Err(send_err) = reply.send(Err(e)) {
                tracing::warn!(agent_id = ?agent_id, error = ?send_err, "GetHistory reply receiver dropped (lock error)");
            }
            return;
        }
    };
    let Some((_ctx, agent_instance)) = instance_opt else {
        if let Err(e) = reply.send(Err(Error::BackendError {
            message: format!("Agent ID {agent_id} not found in registry"),
        })) {
            tracing::warn!(agent_id = ?agent_id, error = ?e, "GetHistory reply receiver dropped (not found)");
        }
        return;
    };

    let result = Python::attach(
        |py| -> Result<Vec<crate::types::ConversationMessage>, Error> {
            let helper_fn = get_or_compile_py_helper(
                &GET_HISTORY_FN,
                PYTHON_GET_HISTORY_SCRIPT,
                GET_HISTORY_FN_NAME,
            )
            .map_err(|e| Error::BackendError { message: e })?;
            let helper_bound = helper_fn.bind(py);

            let agent_bound = agent_instance.bind(py);
            let result = helper_bound.call1((agent_bound,))?;
            let json_str: String = result.extract()?;
            serde_json::from_str(&json_str).map_err(|e| Error::BackendError {
                message: format!("Failed to parse history JSON: {e}"),
            })
        },
    );

    if let Err(e) = reply.send(result) {
        tracing::warn!(agent_id = ?agent_id, error = ?e, "GetHistory reply receiver dropped");
    }
}

/// Query the turn count from the agent's conversation object.
pub(in crate::runtime) fn handle_get_turn_count(
    registry: &AgentRegistry,
    agent_id: AgentId,
    reply: oneshot::Sender<Result<u32, Error>>,
) {
    let instance_opt = match lock_agent_instance(registry, agent_id) {
        Ok(opt) => opt,
        Err(e) => {
            if let Err(send_err) = reply.send(Err(e)) {
                tracing::warn!(agent_id = ?agent_id, error = ?send_err, "GetTurnCount reply receiver dropped (lock error)");
            }
            return;
        }
    };
    let Some((_ctx, agent_instance)) = instance_opt else {
        if let Err(e) = reply.send(Err(Error::BackendError {
            message: format!("Agent ID {agent_id} not found in registry"),
        })) {
            tracing::warn!(agent_id = ?agent_id, error = ?e, "GetTurnCount reply receiver dropped (not found)");
        }
        return;
    };

    let result = Python::attach(|py| -> Result<u32, Error> {
        let helper_fn = get_or_compile_py_helper(
            &GET_TURN_COUNT_FN,
            PYTHON_GET_TURN_COUNT_SCRIPT,
            GET_TURN_COUNT_FN_NAME,
        )
        .map_err(|e| Error::BackendError { message: e })?;
        let helper_bound = helper_fn.bind(py);

        let agent_bound = agent_instance.bind(py);
        let result = helper_bound.call1((agent_bound,))?;
        Ok(result.extract::<u32>()?)
    });

    if let Err(e) = reply.send(result) {
        tracing::warn!(agent_id = ?agent_id, error = ?e, "GetTurnCount reply receiver dropped");
    }
}

/// Query cumulative token usage from the agent's conversation object.
pub(in crate::runtime) fn handle_get_total_usage(
    registry: &AgentRegistry,
    agent_id: AgentId,
    reply: oneshot::Sender<Result<crate::types::UsageMetadata, Error>>,
) {
    handle_get_usage_impl(
        registry,
        agent_id,
        reply,
        &GET_TOTAL_USAGE_FN,
        PYTHON_GET_TOTAL_USAGE_SCRIPT,
        GET_TOTAL_USAGE_FN_NAME,
        "GetTotalUsage",
    );
}

/// Query last-turn token usage from the agent's conversation object.
pub(in crate::runtime) fn handle_get_last_turn_usage(
    registry: &AgentRegistry,
    agent_id: AgentId,
    reply: oneshot::Sender<Result<crate::types::UsageMetadata, Error>>,
) {
    handle_get_usage_impl(
        registry,
        agent_id,
        reply,
        &GET_LAST_TURN_USAGE_FN,
        PYTHON_GET_LAST_TURN_USAGE_SCRIPT,
        GET_LAST_TURN_USAGE_FN_NAME,
        "GetLastTurnUsage",
    );
}

/// Shared implementation for `GetTotalUsage` and `GetLastTurnUsage` commands.
fn handle_get_usage_impl(
    registry: &AgentRegistry,
    agent_id: AgentId,
    reply: oneshot::Sender<Result<crate::types::UsageMetadata, Error>>,
    cache: &'static std::sync::OnceLock<Py<PyAny>>,
    script: &str,
    fn_name: &str,
    label: &str,
) {
    let instance_opt = match lock_agent_instance(registry, agent_id) {
        Ok(opt) => opt,
        Err(e) => {
            if let Err(send_err) = reply.send(Err(e)) {
                tracing::warn!(agent_id = ?agent_id, error = ?send_err, label, "reply receiver dropped (lock error)");
            }
            return;
        }
    };
    let Some((_ctx, agent_instance)) = instance_opt else {
        if let Err(e) = reply.send(Err(Error::BackendError {
            message: format!("Agent ID {agent_id} not found in registry"),
        })) {
            tracing::warn!(agent_id = ?agent_id, label, error = ?e, "reply receiver dropped (not found)");
        }
        return;
    };

    let result = Python::attach(|py| -> Result<crate::types::UsageMetadata, Error> {
        let helper_fn = get_or_compile_py_helper(cache, script, fn_name)
            .map_err(|e| Error::BackendError { message: e })?;
        let helper_bound = helper_fn.bind(py);

        let agent_bound = agent_instance.bind(py);
        let result = helper_bound.call1((agent_bound,))?;
        let json_str: String = result.extract()?;
        serde_json::from_str(&json_str).map_err(|e| Error::BackendError {
            message: format!("Failed to parse usage JSON: {e}"),
        })
    });

    if let Err(e) = reply.send(result) {
        tracing::warn!(agent_id = ?agent_id, label, error = ?e, "reply receiver dropped");
    }
}

/// Return step indices where compaction occurred.
///
/// Queries `agent.conversation.compaction_indices` from the Python SDK.
/// Falls back to an empty list if the attribute is not available.
pub(in crate::runtime) fn handle_get_compaction_indices(
    registry: &AgentRegistry,
    agent_id: AgentId,
    reply: oneshot::Sender<Result<Vec<u32>, Error>>,
) {
    let instance_opt = match lock_agent_instance(registry, agent_id) {
        Ok(opt) => opt,
        Err(e) => {
            if let Err(send_err) = reply.send(Err(e)) {
                tracing::warn!(agent_id = ?agent_id, error = ?send_err, "GetCompactionIndices reply receiver dropped (lock error)");
            }
            return;
        }
    };
    let Some((_ctx, agent_instance)) = instance_opt else {
        if let Err(e) = reply.send(Err(Error::BackendError {
            message: format!("Agent ID {agent_id} not found in registry"),
        })) {
            tracing::warn!(agent_id = ?agent_id, error = ?e, "GetCompactionIndices reply receiver dropped (not found)");
        }
        return;
    };

    let result = Python::attach(|py| -> Result<Vec<u32>, Error> {
        let agent_bound = agent_instance.bind(py);
        if !agent_bound.hasattr("conversation")? {
            return Ok(Vec::new());
        }
        let conv = agent_bound.getattr("conversation")?;
        if !conv.hasattr("compaction_indices")? {
            return Ok(Vec::new());
        }
        let indices = conv.getattr("compaction_indices")?;
        Ok(indices.extract::<Vec<u32>>()?)
    });

    if let Err(e) = reply.send(result) {
        tracing::warn!(agent_id = ?agent_id, error = ?e, "GetCompactionIndices reply receiver dropped");
    }
}

/// Return the text of the last model response.
///
/// Queries the conversation history for the most recent model response text.
/// Returns `None` if no model responses exist.
pub(in crate::runtime) fn handle_get_last_response(
    registry: &AgentRegistry,
    agent_id: AgentId,
    reply: oneshot::Sender<Result<Option<String>, Error>>,
) {
    let instance_opt = match lock_agent_instance(registry, agent_id) {
        Ok(opt) => opt,
        Err(e) => {
            if let Err(send_err) = reply.send(Err(e)) {
                tracing::warn!(agent_id = ?agent_id, error = ?send_err, "GetLastResponse reply receiver dropped (lock error)");
            }
            return;
        }
    };
    let Some((_ctx, agent_instance)) = instance_opt else {
        if let Err(e) = reply.send(Err(Error::BackendError {
            message: format!("Agent ID {agent_id} not found in registry"),
        })) {
            tracing::warn!(agent_id = ?agent_id, error = ?e, "GetLastResponse reply receiver dropped (not found)");
        }
        return;
    };

    let result = Python::attach(|py| -> Result<Option<String>, Error> {
        let agent_bound = agent_instance.bind(py);
        if !agent_bound.hasattr("conversation")? {
            return Ok(None);
        }
        let conv = agent_bound.getattr("conversation")?;
        if !conv.hasattr("last_response")? {
            return Ok(None);
        }
        let response_str: String = conv.getattr("last_response")?.extract()?;
        if response_str.is_empty() {
            return Ok(None);
        }
        Ok(Some(response_str))
    });

    if let Err(e) = reply.send(result) {
        tracing::warn!(agent_id = ?agent_id, error = ?e, "GetLastResponse reply receiver dropped");
    }
}

/// Check whether the agent is currently idle (not running a turn).
///
/// Queries `agent.conversation.is_idle` from the Python SDK. Returns `true`
/// if the conversation object is not present or the attribute is missing.
pub(in crate::runtime) fn handle_is_idle(
    registry: &AgentRegistry,
    agent_id: AgentId,
    reply: oneshot::Sender<Result<bool, Error>>,
) {
    let instance_opt = match lock_agent_instance(registry, agent_id) {
        Ok(opt) => opt,
        Err(e) => {
            if let Err(send_err) = reply.send(Err(e)) {
                tracing::warn!(agent_id = ?agent_id, error = ?send_err, "IsIdle reply receiver dropped (lock error)");
            }
            return;
        }
    };
    let Some((_ctx, agent_instance)) = instance_opt else {
        if let Err(e) = reply.send(Err(Error::BackendError {
            message: format!("Agent ID {agent_id} not found in registry"),
        })) {
            tracing::warn!(agent_id = ?agent_id, error = ?e, "IsIdle reply receiver dropped (not found)");
        }
        return;
    };

    let result = Python::attach(|py| -> Result<bool, Error> {
        let helper_fn =
            get_or_compile_py_helper(&IS_IDLE_FN, PYTHON_IS_IDLE_SCRIPT, IS_IDLE_FN_NAME)
                .map_err(|e| Error::BackendError { message: e })?;
        let helper_bound = helper_fn.bind(py);

        let agent_bound = agent_instance.bind(py);
        let result = helper_bound.call1((agent_bound,))?;
        Ok(result.extract::<bool>()?)
    });

    if let Err(e) = reply.send(result) {
        tracing::warn!(agent_id = ?agent_id, error = ?e, "IsIdle reply receiver dropped");
    }
}

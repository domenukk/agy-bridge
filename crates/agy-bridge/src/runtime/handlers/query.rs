/// Synchronous query handlers that do not spawn background tasks.
use pyo3::prelude::*;
use tokio::sync::oneshot;

use super::super::{AgentId, command_loop::AgentRegistry};
use crate::error::Error;

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
            if reply.send(Err(e)).is_err() {
                tracing::warn!(agent_id = ?agent_id, "get_history reply receiver dropped (lock error)");
            }
            return;
        }
    };
    let Some((_ctx, agent_instance)) = instance_opt else {
        if reply
            .send(Err(Error::BackendError {
                message: format!("Agent ID {agent_id} not found in registry"),
            }))
            .is_err()
        {
            tracing::warn!(agent_id = ?agent_id, "get_history reply receiver dropped (not found)");
        }
        return;
    };

    let result = Python::attach(
        |py| -> Result<Vec<crate::types::ConversationMessage>, Error> {
            let agent_bound = agent_instance.bind(py);
            if !agent_bound.hasattr("conversation")? {
                return Ok(Vec::new());
            }
            let conv = agent_bound.getattr("conversation")?;
            if conv.is_none() || !conv.hasattr("history")? {
                return Ok(Vec::new());
            }
            let history_py = conv.getattr("history")?;
            let history_list = history_py.cast::<pyo3::types::PyList>().map_err(|e| {
                pyo3::exceptions::PyTypeError::new_err(format!(
                    "history attribute is not a list: {e}"
                ))
            })?;
            let mut messages = Vec::new();
            for item in history_list.iter() {
                // Extract role
                let source_py = item.getattr("source")?;
                let role_str = if source_py.hasattr("value")? {
                    source_py.getattr("value")?.extract::<String>()?
                } else if source_py.hasattr("name")? {
                    source_py.getattr("name")?.extract::<String>()?
                } else {
                    source_py
                        .extract::<String>()
                        .unwrap_or_else(|e| {
                            tracing::warn!(agent_id = ?agent_id, error = %e, "failed to extract role, defaulting to 'unknown'");
                            "unknown".to_owned()
                        })
                };
                let role = match role_str.to_lowercase().as_str() {
                    "user" => crate::types::MessageRole::User,
                    "model" => crate::types::MessageRole::Model,
                    "system" => crate::types::MessageRole::System,
                    other => crate::types::MessageRole::Unknown(other.to_owned()),
                };

                // Extract content
                let content = item
                    .getattr("content")?
                    .extract::<String>()
                    .unwrap_or_else(|e| {
                        tracing::warn!(agent_id = ?agent_id, error = %e, "failed to extract message content, defaulting to empty");
                        String::new()
                    });
                messages.push(crate::types::ConversationMessage { role, content });
            }
            Ok(messages)
        },
    );

    if reply.send(result).is_err() {
        tracing::warn!(agent_id = ?agent_id, "get_history reply receiver dropped");
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
            if reply.send(Err(e)).is_err() {
                tracing::warn!(agent_id = ?agent_id, "get_turn_count reply receiver dropped (lock error)");
            }
            return;
        }
    };
    let Some((_ctx, agent_instance)) = instance_opt else {
        if reply
            .send(Err(Error::BackendError {
                message: format!("Agent ID {agent_id} not found in registry"),
            }))
            .is_err()
        {
            tracing::warn!(agent_id = ?agent_id, "get_turn_count reply receiver dropped (not found)");
        }
        return;
    };

    let result = Python::attach(|py| -> Result<u32, Error> {
        let agent_bound = agent_instance.bind(py);
        if !agent_bound.hasattr("conversation")? {
            return Ok(0);
        }
        let conv = agent_bound.getattr("conversation")?;
        if conv.is_none() || !conv.hasattr("turn_count")? {
            return Ok(0);
        }
        let tc = conv.getattr("turn_count")?.extract::<u32>()?;
        Ok(tc)
    });

    if reply.send(result).is_err() {
        tracing::warn!(agent_id = ?agent_id, "get_turn_count reply receiver dropped");
    }
}

/// Query cumulative token usage from the agent's conversation object.
pub(in crate::runtime) fn handle_get_total_usage(
    registry: &AgentRegistry,
    agent_id: AgentId,
    reply: oneshot::Sender<Result<crate::types::UsageMetadata, Error>>,
) {
    handle_get_usage_impl(registry, agent_id, reply, "total_usage", "GetTotalUsage");
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
        "last_turn_usage",
        "GetLastTurnUsage",
    );
}

/// Shared implementation for `GetTotalUsage` and `GetLastTurnUsage` commands.
fn handle_get_usage_impl(
    registry: &AgentRegistry,
    agent_id: AgentId,
    reply: oneshot::Sender<Result<crate::types::UsageMetadata, Error>>,
    attribute: &'static str,
    label: &str,
) {
    let instance_opt = match lock_agent_instance(registry, agent_id) {
        Ok(opt) => opt,
        Err(e) => {
            if reply.send(Err(e)).is_err() {
                tracing::warn!(agent_id = ?agent_id, label, "usage reply receiver dropped (lock error)");
            }
            return;
        }
    };
    let Some((_ctx, agent_instance)) = instance_opt else {
        if reply
            .send(Err(Error::BackendError {
                message: format!("Agent ID {agent_id} not found in registry"),
            }))
            .is_err()
        {
            tracing::warn!(agent_id = ?agent_id, label, "usage reply receiver dropped (not found)");
        }
        return;
    };

    let result = Python::attach(|py| -> Result<crate::types::UsageMetadata, Error> {
        let agent_bound = agent_instance.bind(py);
        if !agent_bound.hasattr("conversation")? {
            return Ok(crate::types::UsageMetadata::default());
        }
        let conv = agent_bound.getattr("conversation")?;
        if conv.is_none() || !conv.hasattr(attribute)? {
            return Ok(crate::types::UsageMetadata::default());
        }
        let usage_py = conv.getattr(attribute)?;
        if usage_py.is_none() {
            return Ok(crate::types::UsageMetadata::default());
        }
        let usage_dict = super::super::py_scripts::to_dict_py(&usage_py)?;
        let usage = usage_dict.extract::<crate::types::UsageMetadata>()?;
        Ok(usage)
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
            if reply.send(Err(e)).is_err() {
                tracing::warn!(agent_id = ?agent_id, "get_compaction_indices reply receiver dropped (lock error)");
            }
            return;
        }
    };
    let Some((_ctx, agent_instance)) = instance_opt else {
        if reply
            .send(Err(Error::BackendError {
                message: format!("Agent ID {agent_id} not found in registry"),
            }))
            .is_err()
        {
            tracing::warn!(agent_id = ?agent_id, "get_compaction_indices reply receiver dropped (not found)");
        }
        return;
    };

    let result = Python::attach(|py| -> Result<Vec<u32>, Error> {
        let agent_bound = agent_instance.bind(py);
        if !agent_bound.hasattr("conversation")? {
            return Ok(Vec::new());
        }
        let conv = agent_bound.getattr("conversation")?;
        if conv.is_none() || !conv.hasattr("compaction_indices")? {
            return Ok(Vec::new());
        }
        let indices = conv.getattr("compaction_indices")?;
        Ok(indices.extract::<Vec<u32>>()?)
    });

    if reply.send(result).is_err() {
        tracing::warn!(agent_id = ?agent_id, "get_compaction_indices reply receiver dropped");
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
            if reply.send(Err(e)).is_err() {
                tracing::warn!(agent_id = ?agent_id, "get_last_response reply receiver dropped (lock error)");
            }
            return;
        }
    };
    let Some((_ctx, agent_instance)) = instance_opt else {
        if reply
            .send(Err(Error::BackendError {
                message: format!("Agent ID {agent_id} not found in registry"),
            }))
            .is_err()
        {
            tracing::warn!(agent_id = ?agent_id, "get_last_response reply receiver dropped (not found)");
        }
        return;
    };

    let result = Python::attach(|py| -> Result<Option<String>, Error> {
        let agent_bound = agent_instance.bind(py);
        if !agent_bound.hasattr("conversation")? {
            return Ok(None);
        }
        let conv = agent_bound.getattr("conversation")?;
        if conv.is_none() || !conv.hasattr("last_response")? {
            return Ok(None);
        }
        let response_str: String = conv.getattr("last_response")?.extract()?;
        if response_str.is_empty() {
            return Ok(None);
        }
        Ok(Some(response_str))
    });

    if reply.send(result).is_err() {
        tracing::warn!(agent_id = ?agent_id, "get_last_response reply receiver dropped");
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
            if reply.send(Err(e)).is_err() {
                tracing::warn!(agent_id = ?agent_id, "is_idle reply receiver dropped (lock error)");
            }
            return;
        }
    };
    let Some((_ctx, agent_instance)) = instance_opt else {
        if reply
            .send(Err(Error::BackendError {
                message: format!("Agent ID {agent_id} not found in registry"),
            }))
            .is_err()
        {
            tracing::warn!(agent_id = ?agent_id, "is_idle reply receiver dropped (not found)");
        }
        return;
    };

    let result = Python::attach(|py| -> Result<bool, Error> {
        let agent_bound = agent_instance.bind(py);
        if !agent_bound.hasattr("conversation")? {
            return Ok(true);
        }
        let conv = agent_bound.getattr("conversation")?;
        if conv.is_none() || !conv.hasattr("is_idle")? {
            return Ok(true);
        }
        let is_idle = conv.getattr("is_idle")?.extract::<bool>()?;
        Ok(is_idle)
    });

    if reply.send(result).is_err() {
        tracing::warn!(agent_id = ?agent_id, "is_idle reply receiver dropped");
    }
}

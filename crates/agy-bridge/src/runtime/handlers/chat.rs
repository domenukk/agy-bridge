/// Chat handlers: `handle_chat` and supporting helpers.
use std::time::Duration;

use pyo3::prelude::*;
use tokio::{sync::oneshot, time::timeout};

use super::super::{
    AgentId,
    command_loop::lookup_agent_instance,
    py_scripts::{PYTHON_CHAT_START_SCRIPT, PYTHON_EXTRACT_METADATA_SCRIPT},
};
use crate::error::Error;

/// Dispatch a `Chat` command: look up the agent, spawn the streaming handler.
pub(in crate::runtime) async fn dispatch_chat_command(
    registry: &super::super::command_loop::AgentRegistry,
    agent_id: AgentId,
    prompt: String,
    reply: oneshot::Sender<Result<crate::streaming::ChatResponseHandle, Error>>,
    chat_timeout: Duration,
    active_tasks: &mut futures::stream::FuturesUnordered<futures::future::BoxFuture<'static, ()>>,
    inter_agent_delay: Duration,
) {
    let Some((_ctx, agent_instance)) = lookup_agent_instance(registry, agent_id) else {
        if let Err(e) = reply.send(Err(Error::BackendError {
            message: format!("Agent ID {agent_id} not found in registry"),
        })) {
            tracing::warn!(error = ?e, "Chat reply receiver dropped (agent not found)");
        }
        return;
    };

    let agent_instance = Python::with_gil(|py| agent_instance.clone_ref(py));
    let chat_fut = handle_chat(agent_instance, chat_timeout, agent_id, prompt, reply);
    active_tasks.push(Box::pin(chat_fut));
    // Small delay between successive chat commands to avoid burst requests.
    tokio::time::sleep(inter_agent_delay).await;
}

/// Obtain the async step iterator from the agent's conversation.
///
/// After `agent.chat()` returns a response, this function reaches into the
/// agent's conversation object and calls `receive_steps()` to get the
/// async iterator that yields individual steps.
fn get_step_iterator(agent_instance: &PyObject, response_py: &PyObject) -> PyResult<PyObject> {
    Python::with_gil(|py| {
        let response_bound = response_py.bind(py);
        let agent_bound = agent_instance.bind(py);

        if !agent_bound.hasattr("conversation")? {
            return Err(pyo3::exceptions::PyAttributeError::new_err(
                "Agent object has no attribute conversation",
            ));
        }
        let conv = agent_bound.getattr("conversation")?;
        if !conv.hasattr("receive_steps")? {
            // Try to surface the error message from the response itself.
            if let Ok(error_text) = response_bound.getattr("text")
                && let Ok(desc_str) = error_text.extract::<String>()
            {
                return Err(pyo3::exceptions::PyRuntimeError::new_err(desc_str));
            }
            return Err(pyo3::exceptions::PyAttributeError::new_err(
                "Conversation object has no attribute receive_steps",
            ));
        }
        let steps = conv.call_method0("receive_steps")?;
        Ok(steps.to_object(py))
    })
}

/// Extract usage and structured-output metadata from a Python response.
///
/// Returns `(usage_json, structured_output_json)` — each `None` if absent
/// or if extraction fails.
fn extract_response_metadata(
    response_py: &PyObject,
    agent_instance: &PyObject,
    agent_id: AgentId,
) -> (Option<String>, Option<String>) {
    let result: (Option<String>, Option<String>) = Python::with_gil(|py| {
        let locals = pyo3::types::PyDict::new_bound(py);
        if let Err(e) = py.run_bound(PYTHON_EXTRACT_METADATA_SCRIPT, None, Some(&locals)) {
            tracing::warn!(agent_id = ?agent_id, error = %e, "Failed to run metadata extraction script");
            return (None, None);
        }
        let helper_fn = match locals.get_item("_extract") {
            Ok(Some(func)) => func,
            Ok(None) => {
                tracing::warn!(agent_id = ?agent_id, "Metadata extraction helper '_extract' not found in locals");
                return (None, None);
            }
            Err(e) => {
                tracing::warn!(agent_id = ?agent_id, error = %e, "Failed to look up '_extract' helper");
                return (None, None);
            }
        };
        match helper_fn.call1((response_py.bind(py), agent_instance.bind(py))) {
            Ok(result) => result.extract().unwrap_or_else(|e| {
                tracing::warn!(agent_id = ?agent_id, error = %e, "Failed to extract metadata tuple from Python result");
                (None, None)
            }),
            Err(e) => {
                tracing::warn!(agent_id = ?agent_id, error = %e, "Metadata extraction call failed");
                (None, None)
            }
        }
    });

    tracing::debug!(
        agent_id = ?agent_id,
        usage_json = ?result.0,
        structured_json = ?result.1,
        "Extracted response metadata"
    );
    result
}

/// Deserialize and apply extracted metadata to the streaming writer.
fn apply_response_metadata(
    writer: &crate::streaming::ChatResponseWriter,
    usage_json: Option<String>,
    structured_json: Option<String>,
) {
    if let Some(u_str) = usage_json
        && let Ok(usage) = serde_json::from_str::<crate::types::UsageMetadata>(&u_str)
    {
        writer.set_usage(usage);
    }
    if let Some(s_str) = structured_json
        && let Ok(val) = serde_json::from_str::<serde_json::Value>(&s_str)
    {
        writer.set_structured_output(val);
    }
}

/// Invoke `agent.chat()` via Python, then iterate the response's async chunk
/// iterator on the Rust side, forwarding each chunk through the
/// [`ChatResponseWriter`] channels for true streaming.
pub(crate) async fn handle_chat(
    agent_instance: PyObject,
    chat_timeout: Duration,
    agent_id: AgentId,
    prompt: String,
    reply: oneshot::Sender<Result<crate::streaming::ChatResponseHandle, Error>>,
) {
    tracing::info!(agent_id = ?agent_id, "Live-SDK: Chat command received");

    // Phase 1: Start the chat in Python and get the response object.
    let start_fut = match prepare_chat_start(&agent_instance, &prompt, chat_timeout) {
        Ok(fut) => fut,
        Err(err_msg) => {
            tracing::error!(agent_id = ?agent_id, error = %err_msg, "Failed to create _start_chat coroutine");
            if let Err(e) = reply.send(Err(Error::BackendError { message: err_msg })) {
                tracing::warn!(error = ?e, "Chat reply receiver dropped (start coro error)");
            }
            return;
        }
    };

    let response_py = match timeout(chat_timeout, start_fut).await {
        Ok(Ok(obj)) => obj,
        Ok(Err(e)) => {
            let err: Error = e.into();
            if let Err(e) = reply.send(Err(err)) {
                tracing::warn!(error = ?e, "Chat reply receiver dropped (start error)");
            }
            return;
        }
        Err(_elapsed) => {
            tracing::error!(agent_id = ?agent_id, timeout_secs = chat_timeout.as_secs(), "_start_chat() timed out");
            if let Err(e) = reply.send(Err(Error::Timeout {
                duration: chat_timeout,
                operation: "start_chat".to_string(),
            })) {
                tracing::warn!(error = ?e, "Chat reply receiver dropped (start timeout)");
            }
            return;
        }
    };

    // Phase 2: Get the async chunk iterator from the response/conversation.
    let aiter_py = match get_step_iterator(&agent_instance, &response_py) {
        Ok(it) => it,
        Err(e) => {
            let err: Error = e.into();
            if let Err(e) = reply.send(Err(err)) {
                tracing::warn!(error = ?e, "Chat reply receiver dropped (aiter error)");
            }
            return;
        }
    };

    // Phase 3: Send the handle to the caller so it can start consuming immediately.
    let (writer, handle) = crate::streaming::channel();
    if let Err(e) = reply.send(Ok(handle)) {
        tracing::warn!(error = ?e, "Chat reply receiver dropped");
        return;
    }

    // Phase 4: Stream steps from the Python async iterator through the writer.
    super::super::streaming::stream_steps_to_writer(&writer, chat_timeout, agent_id, &aiter_py)
        .await;

    // Phase 5: Extract final metadata and apply to the writer.
    let (usage_json, structured_json) =
        extract_response_metadata(&response_py, &agent_instance, agent_id);
    apply_response_metadata(&writer, usage_json, structured_json);
}

fn prepare_chat_start(
    agent_instance: &PyObject,
    prompt: &str,
    timeout: std::time::Duration,
) -> Result<impl std::future::Future<Output = PyResult<PyObject>>, String> {
    Python::with_gil(|py| {
        let locals = pyo3::types::PyDict::new_bound(py);
        py.run_bound(&PYTHON_CHAT_START_SCRIPT, Some(&locals), Some(&locals))
            .map_err(|e| e.to_string())?;
        let helper_fn = locals
            .get_item("_start_chat")
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "Failed to define _start_chat helper".to_string())?;

        let agent_bound = agent_instance.bind(py);
        let coro = helper_fn
            .call1((agent_bound, prompt, timeout.as_secs_f64()))
            .map_err(|e| format!("{e}"))?;
        tracing::debug!("_start_chat coroutine created, converting to future");
        pyo3_async_runtimes::tokio::into_future(coro).map_err(|e| format!("{e}"))
    })
}

/// Chat handlers: `handle_chat` and supporting helpers.
use std::time::Duration;

use pyo3::prelude::*;
use tokio::{sync::oneshot, time::timeout};

use super::super::{
    AgentId,
    command_loop::{get_or_compile_py_helper, lookup_agent_instance},
    py_scripts::{PYTHON_CHAT_START_SCRIPT, PYTHON_EXTRACT_METADATA_SCRIPT},
};
use crate::error::Error;

/// Compiled-function cache for `_start_chat`.
static START_CHAT_FN: std::sync::OnceLock<Py<PyAny>> = std::sync::OnceLock::new();
const START_CHAT_FN_NAME: &str = "_start_chat";

/// Compiled-function cache for `_extract`.
static EXTRACT_METADATA_FN: std::sync::OnceLock<Py<PyAny>> = std::sync::OnceLock::new();
const EXTRACT_METADATA_FN_NAME: &str = "_extract";

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

    let agent_instance = Python::attach(|py| agent_instance.clone_ref(py));
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
fn get_step_iterator(agent_instance: &Py<PyAny>, response_py: &Py<PyAny>) -> PyResult<Py<PyAny>> {
    Python::attach(|py| {
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
        Ok(steps.clone().unbind())
    })
}

/// Extract usage and structured-output metadata from a Python response.
///
/// Returns `(usage_json, structured_output_json)` — each `None` if absent
/// or if extraction fails.
fn extract_response_metadata(
    response_py: &Py<PyAny>,
    agent_instance: &Py<PyAny>,
    agent_id: AgentId,
) -> (Option<String>, Option<String>) {
    let helper = match get_or_compile_py_helper(
        &EXTRACT_METADATA_FN,
        PYTHON_EXTRACT_METADATA_SCRIPT,
        EXTRACT_METADATA_FN_NAME,
    ) {
        Ok(f) => f,
        Err(e) => {
            tracing::error!(agent_id = ?agent_id, error = %e, "Failed to compile metadata extraction helper");
            return (None, None);
        }
    };

    let result: (Option<String>, Option<String>) = Python::attach(|py| {
        match helper.bind(py).call1((response_py.bind(py), agent_instance.bind(py))) {
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
    if let Some(u_str) = usage_json {
        match serde_json::from_str::<crate::types::UsageMetadata>(&u_str) {
            Ok(usage) => writer.set_usage(usage),
            Err(e) => tracing::warn!("Failed to deserialize usage metadata: {e}"),
        }
    }
    if let Some(s_str) = structured_json {
        match serde_json::from_str::<serde_json::Value>(&s_str) {
            Ok(val) => writer.set_structured_output(val),
            Err(e) => tracing::warn!("Failed to deserialize structured output: {e}"),
        }
    }
}

/// Invoke `agent.chat()` via Python, then iterate the response's async chunk
/// iterator on the Rust side, forwarding each chunk through the
/// [`ChatResponseWriter`] channels for true streaming.
pub(crate) async fn handle_chat(
    agent_instance: Py<PyAny>,
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
    agent_instance: &Py<PyAny>,
    prompt: &str,
    timeout: std::time::Duration,
) -> Result<impl std::future::Future<Output = PyResult<Py<PyAny>>>, String> {
    // `_start_chat` depends on helper functions (`_decode_prompt`,
    // `_decode_content`) defined in the same script. These must share the same
    // namespace as `_start_chat`'s `__globals__`, so we compile with a single
    // dict for both globals and locals (unlike `get_or_compile_py_helper`,
    // which passes `None` for globals).
    let helper_fn = if let Some(cached) = START_CHAT_FN.get() {
        Python::attach(|py| cached.clone_ref(py))
    } else {
        Python::attach(|py| {
            let ns = pyo3::types::PyDict::new(py);
            let c_script = std::ffi::CString::new(PYTHON_CHAT_START_SCRIPT.as_str())
                .map_err(|e| e.to_string())?;
            py.run(c_script.as_c_str(), Some(&ns), Some(&ns))
                .map_err(|e| e.to_string())?;
            let fn_obj = ns
                .get_item(START_CHAT_FN_NAME)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Failed to define {START_CHAT_FN_NAME} helper"))?;
            let py_obj = fn_obj.clone().unbind();
            if let Err(e) = START_CHAT_FN.set(py_obj.clone_ref(py)) {
                tracing::debug!("START_CHAT_FN cache was already set: {:?}", e);
            }
            Ok::<Py<PyAny>, String>(py_obj)
        })?
    };

    Python::attach(|py| {
        let agent_bound = agent_instance.bind(py);
        let coro = helper_fn
            .bind(py)
            .call1((agent_bound, prompt, timeout.as_secs_f64()))
            .map_err(|e| format!("{e}"))?;
        tracing::debug!("_start_chat coroutine created, converting to future");
        pyo3_async_runtimes::tokio::into_future(coro).map_err(|e| format!("{e}"))
    })
}

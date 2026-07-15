/// Chat handlers: `handle_chat` and supporting helpers.
use std::time::Duration;

use pyo3::prelude::*;
use tokio::sync::oneshot;

use super::super::{AgentId, command_loop::lookup_agent_instance, py_scripts::decode_prompt_py};
use crate::{error::Error, types::UsageMetadata};

/// Dispatch a `Chat` command: look up the agent, spawn the streaming handler.
pub(in crate::runtime) async fn dispatch_chat_command(
    registry: &super::super::command_loop::AgentRegistry,
    agent_id: AgentId,
    prompt: String,
    reply: oneshot::Sender<Result<crate::streaming::ChatResponseHandle, Error>>,
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
    let chat_fut = handle_chat(agent_instance, agent_id, prompt, reply);
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
            match response_bound.getattr("text") {
                Ok(error_text) => {
                    // NOLINT: extract failure means text isn't a string; fall through to generic error
                    if let Ok(desc_str) = error_text.extract::<String>() {
                        return Err(pyo3::exceptions::PyRuntimeError::new_err(desc_str));
                    }
                }
                Err(e) => {
                    tracing::debug!("response has no 'text' attr to surface error: {e}");
                }
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
fn extract_response_metadata(
    response_py: &Py<PyAny>,
    agent_instance: &Py<PyAny>,
    agent_id: AgentId,
) -> (Option<UsageMetadata>, Option<serde_json::Value>) {
    Python::attach(|py| {
        let response_bound = response_py.bind(py);
        let agent_bound = agent_instance.bind(py);

        // Get usage metadata
        // NOLINT: `.ok()` — probing optional Python attr; absence is expected
        let mut usage_py = response_bound.getattr("usage_metadata").ok();
        if (usage_py.is_none() || usage_py.as_ref().unwrap().is_none())
            && let Ok(conv) = agent_bound.getattr("conversation")
            && !conv.is_none()
        {
            // NOLINT: `.ok()` — probing optional Python attr; absence is expected
            usage_py = conv.getattr("last_turn_usage").ok();
        }
        let usage = usage_py.and_then(|ob| {
            if ob.is_none() {
                None
            } else {
                (|| {
                    let dict = super::super::py_scripts::to_dict_py(&ob)
                        .inspect_err(|e| {
                            tracing::debug!(agent_id = ?agent_id, error = %e, "Failed to convert usage_metadata to dict");
                        })
                        // NOLINT: error already logged by inspect_err above
                        .ok()?;
                    dict.extract::<UsageMetadata>()
                        .inspect_err(|e| {
                            tracing::debug!(agent_id = ?agent_id, error = %e, "Failed to extract UsageMetadata from dict");
                        })
                        // NOLINT: error already logged by inspect_err above
                        .ok()
                })()
            }
        });

        // Get structured output
        // NOLINT: `.ok()` — probing optional Python attr; absence is expected
        let structured_py = response_bound.getattr("structured_output").ok();
        let structured = structured_py.and_then(|ob| {
            if ob.is_none() {
                None
            } else {
                (|| {
                    let dict = super::super::py_scripts::to_dict_py(&ob)
                        .inspect_err(|e| {
                            tracing::debug!(agent_id = ?agent_id, error = %e, "Failed to convert structured_output to dict");
                        })
                        // NOLINT: error already logged by inspect_err above
                        .ok()?;
                    match pythonize::depythonize::<serde_json::Value>(&dict) {
                        Ok(val) => Some(val),
                        Err(e) => {
                            tracing::warn!(agent_id = ?agent_id, error = %e, "Failed to depythonize structured output");
                            None
                        }
                    }
                })()
            }
        });

        (usage, structured)
    })
}

/// Deserialize and apply extracted metadata to the streaming writer.
fn apply_response_metadata(
    writer: &crate::streaming::ChatResponseWriter,
    usage: Option<UsageMetadata>,
    structured: Option<serde_json::Value>,
) {
    if let Some(u) = usage {
        writer.set_usage(u);
    }
    if let Some(s) = structured {
        writer.set_structured_output(s);
    }
}

/// Invoke `agent.chat()` via Python, then iterate the response's async chunk
/// iterator on the Rust side, forwarding each chunk through the
/// [`ChatResponseWriter`] channels for true streaming.
pub(crate) async fn handle_chat(
    agent_instance: Py<PyAny>,
    agent_id: AgentId,
    prompt: String,
    reply: oneshot::Sender<Result<crate::streaming::ChatResponseHandle, Error>>,
) {
    tracing::info!(agent_id = ?agent_id, "Live-SDK: Chat command received");

    // Phase 1: Start the chat in Python and get the response object.
    let start_fut = match prepare_chat_start(&agent_instance, &prompt) {
        Ok(fut) => fut,
        Err(err_msg) => {
            tracing::error!(agent_id = ?agent_id, error = %err_msg, "Failed to create chat coroutine");
            if let Err(e) = reply.send(Err(Error::BackendError { message: err_msg })) {
                tracing::warn!(error = ?e, "Chat reply receiver dropped (start coro error)");
            }
            return;
        }
    };

    let response_py = match start_fut.await {
        Ok(obj) => obj,
        Err(e) => {
            let err: Error = e.into();
            if let Err(e) = reply.send(Err(err)) {
                tracing::warn!(error = ?e, "Chat reply receiver dropped (start error)");
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
    super::super::streaming::stream_steps_to_writer(&writer, agent_id, &aiter_py).await;

    // Phase 5: Extract final metadata and apply to the writer.
    let (usage, structured) = extract_response_metadata(&response_py, &agent_instance, agent_id);
    apply_response_metadata(&writer, usage, structured);
}

fn prepare_chat_start(
    agent_instance: &Py<PyAny>,
    prompt: &str,
) -> Result<impl std::future::Future<Output = PyResult<Py<PyAny>>>, String> {
    Python::attach(|py| {
        let agent_bound = agent_instance.bind(py);
        let decoded =
            decode_prompt_py(py, prompt).map_err(|e| format!("Failed to decode prompt: {e}"))?;
        let coro = agent_bound
            .call_method1("chat", (decoded,))
            .map_err(|e| format!("Failed to call agent.chat: {e}"))?;
        tracing::debug!("agent.chat coroutine created, converting to future");
        pyo3_async_runtimes::tokio::into_future(coro).map_err(|e| format!("{e}"))
    })
}

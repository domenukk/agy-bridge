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
    stream_limits: super::super::streaming::StreamLimits,
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
    let chat_fut = handle_chat(agent_instance, agent_id, prompt, reply, stream_limits);
    active_tasks.push(Box::pin(chat_fut));
    // Small delay between successive chat commands to avoid burst requests.
    // Set to Duration::ZERO via the builder to disable entirely.
    if !inter_agent_delay.is_zero() {
        tokio::time::sleep(inter_agent_delay).await;
    }
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

/// Extract usage and structured-output metadata after a chat turn completes.
///
/// Both are read from the agent's `conversation`, which by this point has been
/// fully drained by [`stream_steps_to_writer`]. Structured output is read via
/// the conversation's **synchronous** `get_last_structured_output()` accessor —
/// never the response's `async structured_output()` method. The latter is a
/// coroutine that calls `resolve()` and would issue a *second* `receive_steps()`
/// on an already-drained conversation, tripping the SDK's `Concurrent
/// receive_steps() calls are not supported` guard. Reading the async method as
/// if it were a data attribute is also what produced the spurious
/// "unsupported type method" depythonize failures.
///
/// # Errors
///
/// Returns an error if a present structured-output payload cannot be
/// deserialized, or if the SDK accessor raises. Absent metadata (no
/// conversation, no accessor, or a `None` payload) yields `Ok(None)` — that is
/// the normal case for agents that do not use structured output, not a failure.
fn extract_response_metadata(
    response_py: &Py<PyAny>,
    agent_instance: &Py<PyAny>,
    agent_id: AgentId,
) -> Result<(Option<UsageMetadata>, Option<serde_json::Value>), String> {
    Python::attach(|py| {
        let response_bound = response_py.bind(py);
        let agent_bound = agent_instance.bind(py);

        // The conversation carries both usage and structured output. It can be
        // absent very early in an agent's life; that is not an error.
        let conversation = agent_bound
            .getattr("conversation")
            // NOLINT: `.ok()` — probing optional Python attr; absence is expected.
            .ok()
            .filter(|conv| !conv.is_none());

        let usage = extract_usage_metadata(response_bound, conversation.as_ref(), agent_id);
        let structured = extract_structured_output(conversation.as_ref())?;
        Ok((usage, structured))
    })
}

/// Read accumulated token usage for the turn.
///
/// Prefers the response's `usage_metadata` property and falls back to the
/// conversation's `last_turn_usage`. Absence is expected and yields `None`; a
/// malformed-but-present payload is logged (as a warning) and dropped, since
/// usage is best-effort telemetry that must never fail an otherwise good turn.
fn extract_usage_metadata(
    response_bound: &Bound<'_, PyAny>,
    conversation: Option<&Bound<'_, PyAny>>,
    agent_id: AgentId,
) -> Option<UsageMetadata> {
    // NOLINT: `.ok()` — probing optional Python attr; absence is expected.
    let mut usage_py = response_bound.getattr("usage_metadata").ok();
    if usage_py.as_ref().is_none_or(pyo3::Bound::is_none)
        && let Some(conv) = conversation
    {
        // NOLINT: `.ok()` — probing optional Python attr; absence is expected.
        usage_py = conv.getattr("last_turn_usage").ok();
    }
    let usage_py = usage_py?;
    if usage_py.is_none() {
        return None;
    }
    let dict = super::super::py_scripts::to_dict_py(&usage_py)
        .inspect_err(|e| {
            tracing::warn!(agent_id = ?agent_id, error = %e, "Failed to convert usage_metadata to dict");
        })
        // NOLINT: best-effort telemetry; error already logged by inspect_err above.
        .ok()?;
    dict.extract::<UsageMetadata>()
        .inspect_err(|e| {
            tracing::warn!(agent_id = ?agent_id, error = %e, "Failed to extract UsageMetadata from dict");
        })
        // NOLINT: best-effort telemetry; error already logged by inspect_err above.
        .ok()
}

/// Read the turn's structured output from the conversation's synchronous
/// `get_last_structured_output()` accessor (the parsed payload of the last
/// FINISH step).
///
/// # Errors
///
/// Returns an error if the accessor raises or if a present payload cannot be
/// deserialized into JSON. A missing conversation, a missing accessor, or a
/// `None` payload all yield `Ok(None)`.
fn extract_structured_output(
    conversation: Option<&Bound<'_, PyAny>>,
) -> Result<Option<serde_json::Value>, String> {
    let Some(conv) = conversation else {
        return Ok(None);
    };
    // Feature-probe: mocks and older SDKs may not expose this accessor. Its
    // absence means the capability is unavailable, which is not an error.
    if !conv
        .hasattr("get_last_structured_output")
        .map_err(|e| format!("probing get_last_structured_output: {e}"))?
    {
        return Ok(None);
    }
    let value = conv
        .call_method0("get_last_structured_output")
        .map_err(|e| format!("get_last_structured_output() raised: {e}"))?;
    if value.is_none() {
        return Ok(None);
    }
    super::super::py_scripts::warm_up_lazy_imports(value.py());
    pythonize::depythonize::<serde_json::Value>(&value)
        .map(Some)
        .map_err(|e| format!("failed to deserialize structured output: {e}"))
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
    stream_limits: super::super::streaming::StreamLimits,
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
    let (writer, handle) = crate::streaming::channel_with_buffer(stream_limits.channel_buffer);
    if let Err(e) = reply.send(Ok(handle)) {
        tracing::warn!(error = ?e, "Chat reply receiver dropped");
        return;
    }

    // Phase 4: Stream steps from the Python async iterator through the writer.
    super::super::streaming::stream_steps_to_writer(&writer, agent_id, &aiter_py, stream_limits)
        .await;

    // Phase 5: Extract final metadata and apply to the writer.
    match extract_response_metadata(&response_py, &agent_instance, agent_id) {
        Ok((usage, structured)) => apply_response_metadata(&writer, usage, structured),
        Err(e) => {
            // Never ignore: a metadata-extraction failure is surfaced both in the
            // logs and to the consumer via the stream's error channel, so it is
            // never silently dropped.
            tracing::error!(agent_id = ?agent_id, error = %e, "Failed to extract response metadata");
            if let Err(send_err) = writer
                .send_error(crate::streaming::StreamError {
                    message: format!("response metadata extraction failed: {e}"),
                })
                .await
            {
                tracing::error!(
                    agent_id = ?agent_id,
                    error = %send_err,
                    "Failed to propagate metadata extraction error to consumer",
                );
            }
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile a snippet of Python into a throwaway module for building mocks.
    fn make_module<'py>(py: Python<'py>, code: &str) -> Bound<'py, pyo3::types::PyModule> {
        let code = std::ffi::CString::new(code).expect("test code has no interior nul");
        pyo3::types::PyModule::from_code(py, &code, c"mock.py", c"mock")
            .expect("mock module compiles")
    }

    /// Instantiate `class_name()` from `code` and return it as a bound object.
    fn instantiate<'py>(py: Python<'py>, code: &str, class_name: &str) -> Bound<'py, PyAny> {
        make_module(py, code)
            .getattr(class_name)
            .expect("class exists")
            .call0()
            .expect("class instantiates")
    }

    #[test]
    fn structured_output_returns_parsed_payload() {
        Python::attach(|py| {
            let conv = instantiate(
                py,
                r#"
class Conv:
    def get_last_structured_output(self):
        return {"answer": 42, "items": [1, 2, 3]}
"#,
                "Conv",
            );
            let result = extract_structured_output(Some(&conv)).expect("extraction succeeds");
            assert_eq!(
                result,
                Some(serde_json::json!({"answer": 42, "items": [1, 2, 3]}))
            );
        });
    }

    #[test]
    fn structured_output_none_payload_is_ok_none() {
        Python::attach(|py| {
            let conv = instantiate(
                py,
                r"
class Conv:
    def get_last_structured_output(self):
        return None
",
                "Conv",
            );
            assert_eq!(extract_structured_output(Some(&conv)), Ok(None));
        });
    }

    #[test]
    fn structured_output_missing_conversation_is_ok_none() {
        assert_eq!(extract_structured_output(None), Ok(None));
    }

    #[test]
    fn structured_output_missing_accessor_is_ok_none() {
        Python::attach(|py| {
            // A conversation object that predates the accessor: absence of the
            // capability is not an error.
            let conv = instantiate(
                py,
                r"
class Conv:
    pass
",
                "Conv",
            );
            assert_eq!(extract_structured_output(Some(&conv)), Ok(None));
        });
    }

    #[test]
    fn structured_output_accessor_raising_is_surfaced() {
        Python::attach(|py| {
            let conv = instantiate(
                py,
                r#"
class Conv:
    def get_last_structured_output(self):
        raise RuntimeError("boom")
"#,
                "Conv",
            );
            let err = extract_structured_output(Some(&conv)).expect_err("must not be ignored");
            assert!(err.contains("boom"), "error should surface cause: {err}");
        });
    }

    #[test]
    fn structured_output_non_json_payload_is_surfaced() {
        Python::attach(|py| {
            // Returning a bound method is exactly the shape that produced the
            // original "unsupported type method" failure. It must now surface as
            // an error rather than being silently swallowed.
            let conv = instantiate(
                py,
                r"
class Conv:
    def _helper(self):
        return 1
    def get_last_structured_output(self):
        return self._helper
",
                "Conv",
            );
            let err = extract_structured_output(Some(&conv)).expect_err("must not be ignored");
            assert!(
                err.contains("deserialize"),
                "error should describe the failure: {err}"
            );
        });
    }

    #[test]
    fn response_metadata_reads_conversation_not_async_method() {
        Python::attach(|py| {
            // The real SDK exposes `structured_output` as an async method on the
            // *response*. Touching it as data is the bug being fixed, so the mock
            // fails loudly if the bridge ever calls it.
            let code = r#"
class Response:
    def __init__(self):
        self.usage_metadata = None
    async def structured_output(self):
        raise AssertionError("bridge must not call response.structured_output()")

class Conv:
    def __init__(self):
        self.last_turn_usage = None
    def get_last_structured_output(self):
        return {"verdict": "ok"}

class Agent:
    def __init__(self):
        self.conversation = Conv()
"#;
            let module = make_module(py, code);
            let response = module
                .getattr("Response")
                .unwrap()
                .call0()
                .unwrap()
                .unbind();
            let agent = module.getattr("Agent").unwrap().call0().unwrap().unbind();

            let (usage, structured) =
                extract_response_metadata(&response, &agent, AgentId(1)).expect("extraction ok");
            assert!(usage.is_none(), "no usage configured");
            assert_eq!(structured, Some(serde_json::json!({"verdict": "ok"})));
        });
    }

    #[test]
    fn response_metadata_reads_usage_and_absent_structured() {
        Python::attach(|py| {
            let code = r#"
class Usage:
    def model_dump(self):
        return {
            "prompt_token_count": 10,
            "candidates_token_count": 5,
            "total_token_count": 15,
        }

class Response:
    def __init__(self):
        self.usage_metadata = Usage()

class Conv:
    def __init__(self):
        self.last_turn_usage = None
    def get_last_structured_output(self):
        return None

class Agent:
    def __init__(self):
        self.conversation = Conv()
"#;
            let module = make_module(py, code);
            let response = module
                .getattr("Response")
                .unwrap()
                .call0()
                .unwrap()
                .unbind();
            let agent = module.getattr("Agent").unwrap().call0().unwrap().unbind();

            let (usage, structured) =
                extract_response_metadata(&response, &agent, AgentId(2)).expect("extraction ok");
            assert!(structured.is_none(), "no structured output configured");
            let usage = usage.expect("usage present");
            assert_eq!(usage.prompt_token_count, Some(10));
            assert_eq!(usage.candidates_token_count, Some(5));
            assert_eq!(usage.total_token_count, Some(15));
        });
    }
}

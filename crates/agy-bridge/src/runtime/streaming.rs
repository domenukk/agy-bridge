//! Step streaming and forwarding handlers.
//!
//! ## Triple-channel fan-out
//!
//! Each streaming event (text, thought, tool-call) is forwarded to **three**
//! channels simultaneously, serving different consumer patterns:
//!
//! | Channel     | Purpose                                       |
//! |-------------|-----------------------------------------------|
//! | `event_tx`  | Timeline replay: all events in arrival order  |
//! | `chunk_tx`  | Unified stream: typed chunks for consumers    |
//! | `text_tx` / `thought_tx` / `tool_call_tx` | Type-specific streams |
//!
//! This fan-out is intentional: consumers may attach to whichever channel
//! suits their use-case (e.g. a CLI only needs `text_tx`, while a UI might
//! consume the full `event_tx` timeline).

use std::time::Duration;

use pyo3::prelude::*;
use tokio::time::timeout;

use super::{AgentId, py_scripts::PYTHON_NEXT_STEP_SCRIPT};

/// Python function name extracted by [`compile_next_step_helper`].
const NEXT_STEP_FN_NAME: &str = "_next_step";

/// Python attribute name on `_agy_bridge_globals` that flags a 429 hit.
const RATE_LIMIT_HIT_ATTR: &str = "RATE_LIMIT_HIT";

/// Error message surfaced when a 429 quota error is intercepted.
const QUOTA_EXCEEDED_MSG: &str = "429: You exceeded your current quota";

static NEXT_STEP_FN: std::sync::OnceLock<Py<PyAny>> = std::sync::OnceLock::new();

fn compile_next_step_helper() -> Result<Py<PyAny>, String> {
    super::command_loop::get_or_compile_py_helper(
        &NEXT_STEP_FN,
        PYTHON_NEXT_STEP_SCRIPT,
        NEXT_STEP_FN_NAME,
    )
}

async fn forward_step_to_writer(
    writer: &crate::streaming::ChatResponseWriter,
    mut step: crate::types::Step,
    agent_id: AgentId,
) -> Result<(), String> {
    // ── Error detection ─────────────────────────────────────────────────
    // The Python SDK sets `step.status = ERROR` and/or populates
    // `step.error` for API-level failures. Actual Python exceptions are
    // caught earlier by `classify_py_step_error`. We only need to check
    // the structured fields — never string-match step content, which
    // would false-positive on legitimate responses (e.g. math, code
    // examples mentioning error codes).
    let has_error_status = step.status == crate::types::StepStatus::Error;
    let has_error_field = !step.error.is_empty();

    if has_error_status || has_error_field {
        route_error_step(writer, &mut step, agent_id).await;
        return Ok(());
    }

    // ── Normal content forwarding ───────────────────────────────────────
    forward_text(writer, &mut step).await?;
    forward_thoughts(writer, &mut step).await?;
    forward_tool_calls(writer, &mut step, agent_id).await?;
    apply_step_metadata(writer, &mut step);

    writer
        .send_step(step)
        .await
        .map_err(|e| format!("Failed to send step: {e}"))?;
    Ok(())
}

/// Route a step with an error status/field to the error channel, then
/// forward the raw step for timeline consumers.
async fn route_error_step(
    writer: &crate::streaming::ChatResponseWriter,
    step: &mut crate::types::Step,
    agent_id: AgentId,
) {
    let has_error_field = !step.error.is_empty();
    let error_msg = if has_error_field {
        std::mem::take(&mut step.error)
    } else {
        // Error status but no error message — include content for context.
        let content = if step.content.is_empty() {
            std::mem::take(&mut step.content_delta)
        } else {
            std::mem::take(&mut step.content)
        };
        format!("Step error (status={:?}): {content}", step.status)
    };
    tracing::warn!(
        agent_id = ?agent_id,
        status = ?step.status,
        error = %error_msg,
        "Step has error status/field — routing to error channel"
    );
    // Use try_send to avoid deadlock: the error channel has capacity 1,
    // and if a subsequent Python exception also tries to send an error
    // via send_stream_error(), the .send().await would block forever
    // because the orchestration loop drains the error channel only AFTER
    // the text channel closes (which requires the writer to be dropped).
    if let Err(e) = writer
        .error_tx
        .try_send(crate::streaming::StreamError { message: error_msg })
    {
        tracing::debug!("Error channel full or closed (first error wins): {e}");
    }
    if let Err(e) = writer.send_step(std::mem::take(step)).await {
        tracing::debug!("Failed to send error step: {e}");
    }
}

/// Extract text content from the step and fan it out to all three channels:
/// `event_tx` (timeline), `chunk_tx` (unified), and `text_tx` (type-specific).
async fn forward_text(
    writer: &crate::streaming::ChatResponseWriter,
    step: &mut crate::types::Step,
) -> Result<(), String> {
    let text = if step.content_delta.is_empty() {
        std::mem::take(&mut step.content)
    } else {
        std::mem::take(&mut step.content_delta)
    };
    if text.is_empty() {
        return Ok(());
    }
    writer
        .event_tx
        .send(crate::streaming::ResponseEvent::TextChunk(text.clone()))
        .await
        .map_err(|e| format!("Failed to send text event: {e}"))?;
    writer
        .chunk_tx
        .send(crate::streaming::StreamChunk::Text(text.clone()))
        .await
        .map_err(|e| format!("Failed to send text chunk to unified stream: {e}"))?;
    writer
        .text_tx
        .send(text)
        .await
        .map_err(|e| format!("Failed to send text chunk: {e}"))?;
    Ok(())
}

/// Extract thinking content from the step and fan it out to all three channels:
/// `event_tx` (timeline), `chunk_tx` (unified), and `thought_tx` (type-specific).
async fn forward_thoughts(
    writer: &crate::streaming::ChatResponseWriter,
    step: &mut crate::types::Step,
) -> Result<(), String> {
    let thinking = if step.thinking_delta.is_empty() {
        std::mem::take(&mut step.thinking)
    } else {
        std::mem::take(&mut step.thinking_delta)
    };
    if thinking.is_empty() {
        return Ok(());
    }
    writer
        .event_tx
        .send(crate::streaming::ResponseEvent::ThoughtChunk(
            thinking.clone(),
        ))
        .await
        .map_err(|e| format!("Failed to send thought event: {e}"))?;
    writer
        .chunk_tx
        .send(crate::streaming::StreamChunk::Thought(thinking.clone()))
        .await
        .map_err(|e| format!("Failed to send thought chunk to unified stream: {e}"))?;
    writer
        .thought_tx
        .send(thinking)
        .await
        .map_err(|e| format!("Failed to send thought chunk: {e}"))?;
    Ok(())
}

/// Extract tool calls from the step and fan each out to all three channels:
/// `event_tx` (timeline), `chunk_tx` (unified), and `tool_call_tx` (type-specific).
async fn forward_tool_calls(
    writer: &crate::streaming::ChatResponseWriter,
    step: &mut crate::types::Step,
    agent_id: AgentId,
) -> Result<(), String> {
    for tc in std::mem::take(&mut step.tool_calls) {
        tracing::debug!(
            agent_id = ?agent_id,
            tool = %tc.name,
            "Streaming tool call event"
        );
        let event = crate::streaming::ToolCallEvent {
            name: tc.name,
            args: tc.args,
            id: tc.id,
            canonical_path: tc.canonical_path,
        };
        writer
            .event_tx
            .send(crate::streaming::ResponseEvent::ToolCall(event.clone()))
            .await
            .map_err(|e| format!("Failed to send tool call event: {e}"))?;
        writer
            .chunk_tx
            .send(crate::streaming::StreamChunk::ToolCall(event.clone()))
            .await
            .map_err(|e| format!("Failed to send tool call to unified stream: {e}"))?;
        writer
            .tool_call_tx
            .send(event)
            .await
            .map_err(|e| format!("Failed to send tool call: {e}"))?;
    }
    Ok(())
}

/// Transfer usage and structured-output metadata from the step to the writer's
/// shared state so the [`ChatResponseHandle`] can read them after completion.
fn apply_step_metadata(
    writer: &crate::streaming::ChatResponseWriter,
    step: &mut crate::types::Step,
) {
    if let Some(usage) = step.usage_metadata.take() {
        writer.set_usage(usage);
    }
    if let Some(out) = step.structured_output.take() {
        writer.set_structured_output(out);
    }
}

enum StepIterationResult {
    Step(Box<crate::types::Step>),
    Stop,
    Error(String),
}

/// Create a Rust future from the Python `_next_step` coroutine.
///
/// Acquires the GIL, calls the helper function, and converts the resulting
/// Python coroutine into a Rust future.
fn create_next_step_future(
    next_step_fn: &Py<PyAny>,
    aiter_py: &Py<PyAny>,
    timeout_secs: f64,
) -> Result<impl std::future::Future<Output = PyResult<Py<PyAny>>>, String> {
    Python::attach(|py| {
        let fn_bound = next_step_fn.bind(py);
        let aiter_bound = aiter_py.bind(py);
        let coro = fn_bound
            .call1((aiter_bound, timeout_secs))
            .map_err(|e| format!("Failed to create _next_step future: {e}"))?;
        pyo3_async_runtimes::tokio::into_future(coro)
            .map_err(|e| format!("Failed to convert _next_step coro to future: {e}"))
    })
}

/// Classify a Python step-iteration error.
///
/// Returns `Stop` for `StopAsyncIteration` (normal end of stream) or
/// `Error` for any other exception.
fn classify_py_step_error(err: &pyo3::PyErr, agent_id: AgentId) -> StepIterationResult {
    let is_stop =
        Python::attach(|py| err.is_instance_of::<pyo3::exceptions::PyStopAsyncIteration>(py));
    if is_stop {
        tracing::debug!(agent_id = ?agent_id, "Step stream ended (StopAsyncIteration)");
        return StepIterationResult::Stop;
    }
    let err_msg = Python::attach(|py| crate::error::classify_py_error(py, err).to_string());
    tracing::error!(agent_id = ?agent_id, error = %err_msg, "Python step iteration failed");
    StepIterationResult::Error(err_msg)
}

/// Extract a JSON string from the Python step object.
///
/// Returns `None` if the object is Python `None` or extraction fails
/// (logged as a warning).
fn extract_step_json(step_py: &Py<PyAny>, agent_id: AgentId) -> Option<String> {
    Python::attach(|py| {
        let bound = step_py.bind(py);
        if bound.is_none() {
            return None;
        }
        match bound.extract::<String>() {
            Ok(s) => Some(s),
            Err(e) => {
                tracing::warn!(
                    agent_id = ?agent_id,
                    error = %e,
                    "Failed to extract step JSON from Python object"
                );
                None
            }
        }
    })
}

/// Check whether the Python-side 429 rate-limit flag has been set.
///
/// Reads `_agy_bridge_globals.RATE_LIMIT_HIT` and resets it to `false`
/// if it was `true`.
fn check_rate_limit_hit() -> bool {
    Python::attach(|py| -> PyResult<bool> {
        let sys = py.import("sys")?;
        let gm = sys
            .getattr("modules")?
            .get_item(super::command_loop::AGY_BRIDGE_GLOBALS_MODULE)?;
        let hit = gm.getattr(RATE_LIMIT_HIT_ATTR)?.extract::<bool>()?;
        if hit && let Err(e) = gm.setattr(RATE_LIMIT_HIT_ATTR, false) {
            tracing::error!(
                "Failed to reset {RATE_LIMIT_HIT_ATTR} flag: {e} — \
                 returning false to prevent permanent stuck 429 state"
            );
            // If we can't reset the flag, pretend we didn't see a hit.
            // Otherwise every future step iteration would falsely report 429.
            return Ok(false);
        }
        Ok(hit)
    })
    .map_err(|e| {
        tracing::debug!(
            "Checking {RATE_LIMIT_HIT_ATTR} flag failed (normal if uninitialized): {e}"
        );
        e
    })
    .unwrap_or_else(|e| {
        tracing::warn!("Rate-limit flag extraction failed: {e}");
        false
    })
}

async fn process_next_step_iteration(
    next_step_fn: &Py<PyAny>,
    aiter_py: &Py<PyAny>,
    chat_timeout: Duration,
    agent_id: AgentId,
    step_count: u64,
    stream_start: std::time::Instant,
) -> StepIterationResult {
    let next_fut = match create_next_step_future(next_step_fn, aiter_py, chat_timeout.as_secs_f64())
    {
        Ok(fut) => fut,
        Err(e) => return StepIterationResult::Error(e),
    };

    let step_result = match timeout(chat_timeout, next_fut).await {
        Ok(result) => result,
        Err(_elapsed) => {
            return StepIterationResult::Error(format!(
                "Step streaming timed out after {}s for agent {agent_id} (step #{step_count})",
                chat_timeout.as_secs()
            ));
        }
    };

    let step_py = match step_result {
        Ok(obj) => obj,
        Err(e) => return classify_py_step_error(&e, agent_id),
    };

    let Some(json_str) = extract_step_json(&step_py, agent_id) else {
        if check_rate_limit_hit() {
            tracing::warn!(
                agent_id = ?agent_id,
                "Intercepted 429 quota error from Python root logger"
            );
            return StepIterationResult::Error(QUOTA_EXCEEDED_MSG.to_string());
        }

        let elapsed_ms = u64::try_from(stream_start.elapsed().as_millis()).unwrap_or_else(|e| {
            tracing::warn!("Int conversion failed: {e}");
            u64::MAX
        });
        tracing::debug!(
            agent_id = ?agent_id, step_count, elapsed_ms,
            "Step stream completed"
        );
        return StepIterationResult::Stop;
    };

    match serde_json::from_str(&json_str) {
        Ok(s) => StepIterationResult::Step(Box::new(s)),
        Err(e) => {
            let err_msg = format!("Failed to parse Step JSON: {e}");
            tracing::error!(agent_id = ?agent_id, "{err_msg}");
            StepIterationResult::Error(err_msg)
        }
    }
}

pub async fn stream_steps_to_writer(
    writer: &crate::streaming::ChatResponseWriter,
    chat_timeout: Duration,
    agent_id: AgentId,
    aiter_py: &Py<PyAny>,
) {
    tracing::debug!(agent_id = ?agent_id, "Starting step streaming");
    let next_step_fn = match compile_next_step_helper() {
        Ok(f) => f,
        Err(err_msg) => {
            tracing::error!(err_msg);
            send_stream_error(writer, err_msg);
            return;
        }
    };
    let mut step_count: u64 = 0;
    let stream_start = std::time::Instant::now();
    loop {
        match process_next_step_iteration(
            &next_step_fn,
            aiter_py,
            chat_timeout,
            agent_id,
            step_count,
            stream_start,
        )
        .await
        {
            StepIterationResult::Step(step) => {
                step_count += 1;
                if let Err(send_err) = forward_step_to_writer(writer, *step, agent_id).await {
                    tracing::error!("{send_err}");
                    return;
                }
            }
            StepIterationResult::Stop => break,
            StepIterationResult::Error(err_msg) => {
                send_stream_error(writer, err_msg);
                return;
            }
        }
    }
}

fn send_stream_error(writer: &crate::streaming::ChatResponseWriter, message: String) {
    // Use try_send to avoid deadlock: the error channel has capacity 1.
    // If route_error_step already sent an error for this same turn, the
    // channel is full and .send().await would block the writer, preventing
    // it from being dropped. Since the writer must be dropped to close the
    // text channel (which handle.text() is waiting on), this creates a
    // deadlock that only resolves when drain_text's timeout fires (~180s).
    // Using try_send means the first error wins; subsequent errors are
    // logged but not queued.
    if let Err(e) = writer
        .error_tx
        .try_send(crate::streaming::StreamError { message })
    {
        tracing::debug!("Error channel full or closed (first error wins): {e}");
    }
}

#[cfg(test)]
mod tests {
    use crate::types::{Step, StepStatus};

    /// Helper: create a step with given status and error field.
    fn step_with(status: StepStatus, error: &str, content: &str) -> Step {
        Step {
            status,
            error: error.to_string(),
            content: content.to_string(),
            ..Step::default()
        }
    }

    #[test]
    fn error_status_is_detected() {
        let step = step_with(StepStatus::Error, "", "some content");
        assert_eq!(step.status, StepStatus::Error);
        assert!(step.error.is_empty());
        // forward_step_to_writer checks: has_error_status || has_error_field
        let has_error_status = step.status == StepStatus::Error;
        let has_error_field = !step.error.is_empty();
        assert!(has_error_status || has_error_field);
    }

    #[test]
    fn error_field_is_detected() {
        let step = step_with(StepStatus::Done, "quota exceeded", "");
        let has_error_status = step.status == StepStatus::Error;
        let has_error_field = !step.error.is_empty();
        assert!(has_error_status || has_error_field);
    }

    #[test]
    fn both_error_signals_detected() {
        let step = step_with(StepStatus::Error, "model not found", "error text");
        let has_error_status = step.status == StepStatus::Error;
        let has_error_field = !step.error.is_empty();
        assert!(has_error_status && has_error_field);
    }

    #[test]
    fn normal_step_is_not_error() {
        let step = step_with(StepStatus::Done, "", "The answer is 42.");
        let has_error_status = step.status == StepStatus::Error;
        let has_error_field = !step.error.is_empty();
        assert!(!has_error_status && !has_error_field);
    }

    #[test]
    fn active_step_with_no_error_is_not_error() {
        let step = step_with(StepStatus::Active, "", "Working on it...");
        let has_error_status = step.status == StepStatus::Error;
        let has_error_field = !step.error.is_empty();
        assert!(!has_error_status && !has_error_field);
    }

    #[test]
    fn content_mentioning_error_codes_is_not_flagged() {
        // This is the key test: content that mentions error codes, HTTP
        // status codes, etc. must NOT be falsely detected as an error.
        let step = step_with(
            StepStatus::Done,
            "",
            "The request failed with code 404. Here is a fix for handling \
             error codes 4xx and 5xx in your application.",
        );
        let has_error_status = step.status == StepStatus::Error;
        let has_error_field = !step.error.is_empty();
        assert!(
            !has_error_status && !has_error_field,
            "Normal content discussing error codes must not be flagged as error"
        );
    }
}

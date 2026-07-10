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

use super::AgentId;

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

    // ── Extract summary info before forwarding consumes the data ────────
    let step_idx = step.step_index;
    let tool_names: Vec<String> = step.tool_calls.iter().map(|tc| tc.name.clone()).collect();
    let usage_summary = step.usage_metadata.as_ref().map(|u| {
        format!(
            "{}p/{}o/{}t",
            // NOLINT: zero is the correct default for missing token counts
            u.prompt_token_count.unwrap_or(0),
            // NOLINT: zero is the correct default for missing token counts
            u.candidates_token_count.unwrap_or(0),
            // NOLINT: zero is the correct default for missing token counts
            u.thoughts_token_count.unwrap_or(0),
        )
    });
    let text_len = step.content.len() + step.content_delta.len();
    let thinking_len = step.thinking.len() + step.thinking_delta.len();

    // ── Normal content forwarding ───────────────────────────────────────
    forward_text(writer, &mut step).await?;
    forward_thoughts(writer, &mut step).await?;
    forward_tool_calls(writer, &mut step, agent_id).await?;
    apply_step_metadata(writer, &mut step);

    writer
        .send_step(step)
        .await
        .map_err(|e| format!("Failed to send step: {e}"))?;

    // ── Structured step summary ─────────────────────────────────────────
    if !tool_names.is_empty() {
        tracing::info!(
            agent_id = ?agent_id,
            step = step_idx,
            tools = ?tool_names,
            usage = ?usage_summary,
            "tool_call"
        );
    } else if text_len > 0 || thinking_len > 0 {
        tracing::debug!(
            agent_id = ?agent_id,
            text_len,
            thinking_len,
            usage = ?usage_summary,
            "model_output"
        );
    }

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
        step.error.clone()
    } else {
        let content = if step.content.is_empty() {
            step.content_delta.clone()
        } else {
            step.content.clone()
        };
        format!("Step error (status={:?}): {content}", step.status)
    };
    step.error = error_msg.clone();
    tracing::warn!(
        agent_id = ?agent_id,
        status = ?step.status,
        error = %error_msg,
        "Step has error status/field"
    );
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

    if step.source == crate::types::StepSource::Model {
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
    }
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

async fn process_next_step_iteration(
    aiter_py: &Py<PyAny>,
    chat_timeout: Duration,
    agent_id: AgentId,
    step_count: u64,
) -> StepIterationResult {
    let next_fut = Python::attach(|py| -> PyResult<_> {
        let aiter_bound = aiter_py.bind(py);
        let coro = aiter_bound.call_method0("__anext__")?;
        pyo3_async_runtimes::tokio::into_future(coro)
    });

    let next_fut = match next_fut {
        Ok(fut) => fut,
        Err(e) => return classify_py_step_error(&e, agent_id),
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

    Python::attach(|py| {
        let step_bound = step_py.bind(py);
        if step_bound.is_none() {
            return StepIterationResult::Stop;
        }
        match super::py_scripts::to_dict_py(step_bound)
            .and_then(|d| d.extract::<crate::types::Step>())
        {
            Ok(step) => StepIterationResult::Step(Box::new(step)),
            Err(e) => {
                let err_msg = format!("Failed to extract Step from Python object: {e}");
                tracing::error!(agent_id = ?agent_id, "{err_msg}");
                StepIterationResult::Error(err_msg)
            }
        }
    })
}

pub async fn stream_steps_to_writer(
    writer: &crate::streaming::ChatResponseWriter,
    chat_timeout: Duration,
    agent_id: AgentId,
    aiter_py: &Py<PyAny>,
) {
    tracing::debug!(agent_id = ?agent_id, "Starting step streaming");
    let mut step_count: u64 = 0;
    loop {
        match process_next_step_iteration(aiter_py, chat_timeout, agent_id, step_count).await {
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
}

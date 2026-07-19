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
//!
//! ## Subscription gating (deadlock safety)
//!
//! Each channel is only written when a consumer has *subscribed* to it (by
//! calling the matching handle accessor). A channel nobody drains is skipped
//! entirely, so its bounded buffer can never fill and block the writer. This
//! makes every consumption pattern deadlock-free while still delivering every
//! item to the channels that *are* consumed. See `StreamSubscriptions` and
//! `ChatResponseWriter::fan_out`.

use pyo3::prelude::*;

// ── Stream error strategy ─────────────────────────────────────────────────
//
// agy-bridge wraps the Python SDK and does NOT classify individual error
// steps.  The Go backend (localharness) handles retries and decides what's
// fatal vs recoverable.  The Python SDK raises exceptions for fatal errors
// (e.g. `AntigravityExecutionError` after exhausted retries) and yields
// error steps for intermediate failures (which the backend may retry).
//
// We simply:
// 1. Forward every error step to timeline consumers (step channel).
// 2. Track the last error and whether output arrived *after* it.
// 3. At stream end: if no output came after the last error → propagate it
//    to error_tx so handle.text() returns Err(StreamError).
//    If the backend recovered (produced text after the error) → skip it.
//
// This avoids fragile string-matching on backend-specific messages like
// "model output", "terminated", or "Retryable error".
use super::AgentId;

/// What a forwarded step contained — used by the stream loop to track
/// whether the stream produced useful output.
enum StepContent {
    /// The step had text content and/or tool calls.
    Output,
    /// The step was an error (no content produced, error forwarded).
    Error(String),
    /// The step had no content and no error (e.g. metadata-only).
    Empty,
}

/// Maximum number of *consecutive* model-quality error steps tolerated within a
/// single stream before we stop pulling the SDK iterator and fail the turn.
///
/// The Python SDK retries a "thinking-only / invalid output" turn internally by
/// re-issuing the model call, and with some backends each re-issue is a fresh
/// RPC — a brand-new subprocess and network connection. A context that
/// *deterministically* yields invalid output would otherwise retry up to the
/// SDK's own (large) ceiling, spawning a burst of RPCs per turn that can exhaust
/// connection/socket resources. Cutting the stream after a few attempts hands control to the
/// orchestrator's turn-level recovery ladder (drop-the-bad-turn + corrective
/// nudge, then respawn), which breaks the loop far more cheaply — and more
/// effectively — than blind re-generation on the same poisoned context.
///
/// Transient API errors (e.g. HTTP 503) are deliberately *not* counted here: the
/// SDK's backoff/retry for those is genuinely useful and left intact.
const DEFAULT_MAX_CONSECUTIVE_MODEL_ERRORS: u32 = 3;

/// Maximum number of *consecutive* thinking-only / empty steps tolerated within a
/// single generation before we abort the stream.
///
/// Some models can enter a rumination loop: they stream thinking deltas
/// indefinitely without ever emitting text or a tool call, and the generation
/// only terminates with an "invalid output" error much later. While that single
/// stream runs (often minutes), the underlying subprocess keeps opening sockets,
/// so a *single* runaway generation — not just a retry loop — can balloon
/// fd/socket usage into the thousands. Aborting after a generous ceiling of
/// purely-non-productive steps caps the subprocess's lifetime and hands the turn
/// to the orchestrator's recovery ladder.
///
/// The ceiling is deliberately high so that legitimately long chains of thought
/// that *do* eventually produce output (which resets the counter) are never cut.
const DEFAULT_MAX_CONSECUTIVE_EMPTY_STEPS: u32 = 500;

/// Configurable thresholds for stream abort safety valves.
///
/// These limits prevent runaway SDK retry loops and rumination streams from
/// exhausting resources. A value of `0` disables the corresponding limit
/// entirely, giving pure SDK pass-through behavior.
#[derive(Debug, Clone, Copy)]
pub(crate) struct StreamLimits {
    /// Max consecutive model-quality errors before aborting. 0 = unlimited.
    pub max_model_errors: u32,
    /// Max consecutive thinking-only/empty steps before aborting. 0 = unlimited.
    pub max_empty_steps: u32,
    /// Buffer size for streaming response channels.
    pub channel_buffer: usize,
}

impl Default for StreamLimits {
    fn default() -> Self {
        Self {
            max_model_errors: DEFAULT_MAX_CONSECUTIVE_MODEL_ERRORS,
            max_empty_steps: DEFAULT_MAX_CONSECUTIVE_EMPTY_STEPS,
            channel_buffer: crate::streaming::DEFAULT_CHANNEL_BUFFER,
        }
    }
}

impl StreamLimits {
    /// Build from `RuntimeConfig` overrides, falling back to defaults.
    pub fn from_config(config: &super::config::RuntimeConfig) -> Self {
        Self {
            max_model_errors: config
                .max_consecutive_model_errors
                .unwrap_or(DEFAULT_MAX_CONSECUTIVE_MODEL_ERRORS),
            max_empty_steps: config
                .max_consecutive_empty_steps
                .unwrap_or(DEFAULT_MAX_CONSECUTIVE_EMPTY_STEPS),
            channel_buffer: config
                .streaming_channel_buffer
                .unwrap_or(crate::streaming::DEFAULT_CHANNEL_BUFFER),
        }
    }
}

/// Synthetic error recorded when a generation is aborted for producing only
/// thinking/empty steps. Contains the "model output" marker so it is classified
/// as a model-quality failure by [`is_model_quality_error`] and routed through
/// the same recovery path as a backend-reported invalid-output error.
const RUNAWAY_THINKING_ERROR: &str = "aborted: model output contained only thinking with no text or tool calls \
     after too many consecutive steps (runaway rumination)";

/// Whether an error message denotes a model-*quality* failure (empty /
/// thought-only / invalid tool-call output) as opposed to a transient transport
/// or API error. Kept as a single predicate so the streaming loop and the
/// per-step logging agree on the classification.
fn is_model_quality_error(message: &str) -> bool {
    message.contains("model output")
}

/// Running state of the end-of-stream error state machine.
///
/// Tracks the last error seen, whether useful output arrived *after* it (which
/// makes the error stale), how many model-quality errors have occurred back to
/// back (used to break out of an SDK re-generation loop early), and how many
/// thinking-only steps have occurred back to back (used to abort a single
/// runaway rumination stream).
#[derive(Default)]
struct StreamErrorState {
    last_error: Option<String>,
    output_after_error: bool,
    consecutive_model_errors: u32,
    consecutive_empty_steps: u32,
    limits: StreamLimits,
}

impl StreamErrorState {
    fn new(limits: StreamLimits) -> Self {
        Self {
            limits,
            ..Self::default()
        }
    }

    /// Fold one step's classified [`StepContent`] into the state.
    ///
    /// Returns `true` when the caller should stop pulling the iterator, either
    /// because too many consecutive model-quality errors have occurred (the SDK
    /// is re-generating a deterministically-bad turn) or because a single
    /// generation has produced too many consecutive thinking-only steps (a
    /// runaway rumination stream).
    fn observe(&mut self, content: &StepContent) -> bool {
        match content {
            StepContent::Error(msg) => {
                // An error breaks any thinking-only streak.
                self.consecutive_empty_steps = 0;
                if is_model_quality_error(msg) {
                    self.consecutive_model_errors += 1;
                } else {
                    // A transport/API error breaks the model-quality streak;
                    // the SDK's own retry policy governs those.
                    self.consecutive_model_errors = 0;
                }
                self.last_error = Some(msg.clone());
                self.output_after_error = false;
                self.limits.max_model_errors > 0
                    && self.consecutive_model_errors >= self.limits.max_model_errors
            }
            StepContent::Output => {
                // Any usable output means the turn is making progress: forget
                // both streaks and mark the last error (if any) stale.
                self.consecutive_model_errors = 0;
                self.consecutive_empty_steps = 0;
                if self.last_error.is_some() {
                    self.output_after_error = true;
                }
                false
            }
            StepContent::Empty => {
                self.consecutive_empty_steps += 1;
                if self.limits.max_empty_steps > 0
                    && self.consecutive_empty_steps >= self.limits.max_empty_steps
                {
                    // Record a synthetic model-quality error so the end-of-stream
                    // logic propagates it and the orchestrator recovers.
                    self.last_error = Some(RUNAWAY_THINKING_ERROR.to_string());
                    self.output_after_error = false;
                    true
                } else {
                    false
                }
            }
        }
    }
}

async fn forward_step_to_writer(
    writer: &crate::streaming::ChatResponseWriter,
    mut step: crate::types::Step,
    agent_id: AgentId,
    streamed_text: &mut String,
) -> StepContent {
    // ── Error detection ─────────────────────────────────────────────────
    // The Python SDK sets `step.status = ERROR` and/or populates
    // `step.error` for API-level failures.  Actual Python exceptions are
    // caught earlier by `classify_py_step_error`.  We only need to check
    // the structured fields — never string-match step content.
    let has_error_status = step.status == crate::types::StepStatus::Error;
    let has_error_field = !step.error.is_empty();

    if has_error_status || has_error_field {
        // Log for observability, forward to step channel for timeline
        // consumers, but do NOT break the stream.  The SDK decides when
        // the iterator is done (StopAsyncIteration or exception).
        let error_msg = format_error_message(&step);
        let http_code = step.http_code;
        let is_model_quality = is_model_quality_error(&error_msg);
        tracing::warn!(
            agent_id = ?agent_id,
            http_code,
            error = %error_msg,
            "{}",
            if is_model_quality {
                "Model produced invalid output. Stream continues (backend will retry)"
            } else {
                "Error step received. Stream continues (backend controls iteration)"
            }
        );
        // Do NOT eagerly send to error_tx here: the backend may retry after
        // a recoverable error and produce valid text in a subsequent step.
        // The end-of-stream logic in `stream_steps_to_writer` tracks
        // whether output arrived *after* the last error and only sends
        // to error_tx if no output followed (i.e. the error was fatal).
        // Forward the error step so timeline consumers see it.
        crate::streaming::ChatResponseWriter::fan_out(
            &writer.subs.step,
            &writer.step_tx,
            std::mem::take(&mut step),
            "step",
        )
        .await;
        return StepContent::Error(error_msg);
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
    let has_tool_calls = !step.tool_calls.is_empty();
    // A "complete response" step marks the end of a model message. The SDK may
    // emit it *in addition to* the incremental delta steps, repeating the full
    // text; once handled, reset the per-message dedup accumulator so the next
    // message measures duplicates afresh.
    let is_complete_response = step.is_complete_response == Some(true);

    // ── Normal content forwarding ───────────────────────────────────────
    // Every fan-out is subscription-gated and non-fatal: a view nobody drains
    // is skipped (so it can never fill its buffer and block the writer), and a
    // dropped receiver never aborts the stream.
    forward_text(writer, &mut step, streamed_text).await;
    if is_complete_response {
        streamed_text.clear();
    }
    forward_thoughts(writer, &mut step).await;
    forward_tool_calls(writer, &mut step, agent_id).await;
    apply_step_metadata(writer, &mut step);

    crate::streaming::ChatResponseWriter::fan_out(&writer.subs.step, &writer.step_tx, step, "step")
        .await;

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

    if text_len > 0 || has_tool_calls {
        StepContent::Output
    } else {
        StepContent::Empty
    }
}

/// Extract a human-readable error message from a step's error fields.
fn format_error_message(step: &crate::types::Step) -> String {
    if !step.error.is_empty() {
        return step.error.clone();
    }
    let content = if step.content.is_empty() {
        &step.content_delta
    } else {
        &step.content
    };
    format!("Step error (status={:?}): {content}", step.status)
}

/// Extract text content from the step and fan it out to the (subscribed)
/// `event_tx` (timeline), `chunk_tx` (unified), and `text_tx` (type-specific)
/// channels. Unsubscribed views are skipped; no send can block or fail fatally.
async fn forward_text(
    writer: &crate::streaming::ChatResponseWriter,
    step: &mut crate::types::Step,
    streamed_text: &mut String,
) {
    let is_model = step.source == crate::types::StepSource::Model;
    let (raw, is_delta) = if step.content_delta.is_empty() {
        (std::mem::take(&mut step.content), false)
    } else {
        (std::mem::take(&mut step.content_delta), true)
    };
    if raw.is_empty() {
        return;
    }

    // For model output, de-duplicate a consolidated "complete response" step
    // that repeats text already streamed via deltas within the same message.
    // The SDK yields *both* the incremental deltas and a final full-content
    // step per turn; forwarding both would double the response text.
    let text = if is_model {
        match dedup_model_text(raw, is_delta, streamed_text) {
            Some(t) => t,
            None => return,
        }
    } else {
        raw
    };

    crate::streaming::ChatResponseWriter::fan_out(
        &writer.subs.event,
        &writer.event_tx,
        crate::streaming::ResponseEvent::TextChunk(text.clone()),
        "event",
    )
    .await;

    if is_model {
        crate::streaming::ChatResponseWriter::fan_out(
            &writer.subs.chunk,
            &writer.chunk_tx,
            crate::streaming::StreamChunk::Text(text.clone()),
            "chunk",
        )
        .await;
        crate::streaming::ChatResponseWriter::fan_out(
            &writer.subs.text,
            &writer.text_tx,
            text,
            "text",
        )
        .await;
    }
}

/// Compute the *new* model text to stream, de-duplicating a consolidated
/// full-content step against text already streamed via deltas within the same
/// model message. Returns `None` when the step carries no new text.
///
/// The Antigravity SDK emits, per turn, incremental delta steps *and* a final
/// "complete response" step whose `content` repeats the full message. Emitting
/// both doubles the response; this reconciles them:
/// - a delta is always new text;
/// - a full-content step is skipped if it merely repeats the streamed deltas,
///   or trimmed to its new tail if it grew beyond them;
/// - with no prior deltas (non-streaming turn) the content is emitted as-is.
fn dedup_model_text(raw: String, is_delta: bool, streamed: &mut String) -> Option<String> {
    if is_delta {
        // Incremental token: always new text.
        streamed.push_str(&raw);
        return Some(raw);
    }
    if streamed.is_empty() {
        // Non-streaming turn (content only, no prior deltas): emit as-is.
        streamed.push_str(&raw);
        return Some(raw);
    }
    if raw == *streamed {
        // Exact consolidation of the streamed deltas — nothing new.
        return None;
    }
    if let Some(suffix) = raw.strip_prefix(streamed.as_str()) {
        // Content grew beyond what we streamed: emit only the new tail.
        let suffix = suffix.to_owned();
        streamed.push_str(&suffix);
        return Some(suffix);
    }
    // Unrelated full content (e.g. a fresh message without deltas): emit it and
    // reset the baseline so any later consolidation is measured against it.
    streamed.clear();
    streamed.push_str(&raw);
    Some(raw)
}

/// Extract thinking content from the step and fan it out to the (subscribed)
/// `event_tx` (timeline), `chunk_tx` (unified), and `thought_tx` (type-specific)
/// channels. Unsubscribed views are skipped; no send can block or fail fatally.
async fn forward_thoughts(
    writer: &crate::streaming::ChatResponseWriter,
    step: &mut crate::types::Step,
) {
    let thinking = if step.thinking_delta.is_empty() {
        std::mem::take(&mut step.thinking)
    } else {
        std::mem::take(&mut step.thinking_delta)
    };
    if thinking.is_empty() {
        return;
    }
    crate::streaming::ChatResponseWriter::fan_out(
        &writer.subs.event,
        &writer.event_tx,
        crate::streaming::ResponseEvent::ThoughtChunk(thinking.clone()),
        "event",
    )
    .await;
    crate::streaming::ChatResponseWriter::fan_out(
        &writer.subs.chunk,
        &writer.chunk_tx,
        crate::streaming::StreamChunk::Thought(thinking.clone()),
        "chunk",
    )
    .await;
    crate::streaming::ChatResponseWriter::fan_out(
        &writer.subs.thought,
        &writer.thought_tx,
        thinking,
        "thought",
    )
    .await;
}

/// Extract tool calls from the step and fan each out to the (subscribed)
/// `event_tx` (timeline), `chunk_tx` (unified), and `tool_call_tx`
/// (type-specific) channels. Unsubscribed views are skipped; no send can block
/// or fail fatally.
async fn forward_tool_calls(
    writer: &crate::streaming::ChatResponseWriter,
    step: &mut crate::types::Step,
    agent_id: AgentId,
) {
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
        crate::streaming::ChatResponseWriter::fan_out(
            &writer.subs.event,
            &writer.event_tx,
            crate::streaming::ResponseEvent::ToolCall(event.clone()),
            "event",
        )
        .await;
        crate::streaming::ChatResponseWriter::fan_out(
            &writer.subs.chunk,
            &writer.chunk_tx,
            crate::streaming::StreamChunk::ToolCall(event.clone()),
            "chunk",
        )
        .await;
        crate::streaming::ChatResponseWriter::fan_out(
            &writer.subs.tool_call,
            &writer.tool_call_tx,
            event,
            "tool_call",
        )
        .await;
    }
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
    agent_id: AgentId,
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

    let step_py = match next_fut.await {
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
    agent_id: AgentId,
    aiter_py: &Py<PyAny>,
    limits: StreamLimits,
) {
    tracing::debug!(agent_id = ?agent_id, ?limits, "Starting step streaming");

    // Track the last error and whether useful output arrived *after* it.
    // If the SDK recovered (produced text/tool-calls after an error step),
    // the error is stale and should not be propagated.
    let mut state = StreamErrorState::new(limits);
    // Accumulates model text already streamed within the current message so a
    // consolidated "complete response" step is not re-emitted on top of its
    // deltas (which would double the response text).
    let mut streamed_text = String::new();

    loop {
        match process_next_step_iteration(aiter_py, agent_id).await {
            StepIterationResult::Step(step) => {
                let content =
                    forward_step_to_writer(writer, *step, agent_id, &mut streamed_text).await;
                if state.observe(&content) {
                    // Either the SDK is re-generating a deterministically-bad
                    // turn in a tight loop (each attempt a fresh RPC /
                    // subprocess with some backends), or a single generation
                    // is ruminating without ever producing output. Both hold a
                    // subprocess open and churn sockets. Stop pulling the
                    // iterator and let the orchestrator's turn-level recovery
                    // ladder (drop-the-bad-turn + corrective nudge, then respawn)
                    // break the loop instead of streaming/re-generating forever.
                    tracing::warn!(
                        agent_id = ?agent_id,
                        consecutive_model_errors = state.consecutive_model_errors,
                        consecutive_empty_steps = state.consecutive_empty_steps,
                        "Stopping stream (repeated invalid output or runaway \
                         thinking-only rumination) — handing off to orchestrator recovery"
                    );
                    break;
                }
            }
            StepIterationResult::Stop => break,
            StepIterationResult::Error(err_msg) => {
                // Python exception (not a step) — always fatal.
                send_stream_error(writer, err_msg);
                return;
            }
        }
    }

    // ── Stream ended (iterator exhausted, or cut short after repeated
    // invalid model output) ─────────────────────────────────────────
    // Propagate the last error only if no useful output followed it. If the
    // SDK recovered (produced text/tool-calls after the error), the error is
    // stale and the stream effectively succeeded.
    if let Some(error_msg) = state.last_error {
        if state.output_after_error {
            tracing::info!(
                agent_id = ?agent_id,
                error = %error_msg,
                "Stream recovered after error — not propagating"
            );
        } else {
            tracing::warn!(
                agent_id = ?agent_id,
                error = %error_msg,
                "Stream ended with unrecovered error — propagating"
            );
            send_stream_error(writer, error_msg);
        }
    }
}

fn send_stream_error(writer: &crate::streaming::ChatResponseWriter, message: String) {
    // Use try_send to avoid deadlock: the error channel has capacity 1.
    // The writer must be dropped to close the text channel (which
    // handle.text() is waiting on). If error_tx is already full, the
    // first error wins; subsequent errors are logged but not queued.
    if let Err(e) = writer
        .error_tx
        .try_send(crate::streaming::StreamError { message })
    {
        tracing::debug!("Error channel full or closed (first error wins): {e}");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use super::{AgentId, dedup_model_text, format_error_message, forward_step_to_writer};
    use crate::types::{Step, StepSource, StepStatus};

    /// Helper: create a step with given status and error field.
    fn step_with(status: StepStatus, error: &str, content: &str) -> Step {
        Step {
            status,
            error: error.to_string(),
            content: content.to_string(),
            ..Step::default()
        }
    }

    // ── model-text de-duplication tests ──────────────────────────────
    //
    // The SDK emits, per turn, streaming delta steps *and* a consolidated
    // "complete response" step that repeats the full text. Forwarding both
    // doubled the response under concurrency (a load-dependent flake). These
    // tests pin the reconciliation done by `forward_text` / `dedup_model_text`.

    /// Build a MODEL step carrying an incremental delta.
    fn model_delta(delta: &str) -> Step {
        Step {
            source: StepSource::Model,
            content_delta: delta.to_string(),
            ..Step::default()
        }
    }

    /// Build a MODEL "complete response" step carrying full content.
    fn model_complete(content: &str) -> Step {
        Step {
            source: StepSource::Model,
            content: content.to_string(),
            is_complete_response: Some(true),
            ..Step::default()
        }
    }

    /// Forward all steps through one accumulator and drain the text channel.
    async fn text_of(steps: Vec<Step>) -> String {
        let (writer, handle) = crate::streaming::channel();
        writer.subs.text.store(true, Ordering::Release);
        let mut streamed = String::new();
        for step in steps {
            forward_step_to_writer(&writer, step, AgentId(1), &mut streamed).await;
        }
        drop(writer);
        handle.text().await.expect("text drains cleanly").into()
    }

    /// Regression: a delta step followed by a consolidated complete-response
    /// step repeating the same text must yield the text exactly once — this is
    /// the exact doubling observed under concurrent load.
    #[tokio::test]
    async fn consolidated_complete_response_is_not_double_emitted() {
        let text = text_of(vec![
            model_delta("Healthy mock response"),
            model_complete("Healthy mock response"),
        ])
        .await;
        assert_eq!(text, "Healthy mock response");
    }

    /// Incremental deltas concatenate, and the trailing consolidation that
    /// repeats their sum is dropped.
    #[tokio::test]
    async fn incremental_deltas_concatenate_once() {
        let text = text_of(vec![
            model_delta("Heal"),
            model_delta("thy "),
            model_delta("mock "),
            model_delta("response"),
            model_complete("Healthy mock response"),
        ])
        .await;
        assert_eq!(text, "Healthy mock response");
    }

    /// A non-streaming turn (content only, no deltas) is emitted once.
    #[tokio::test]
    async fn non_streaming_single_content_step_emitted_once() {
        let text = text_of(vec![model_complete("Only once")]).await;
        assert_eq!(text, "Only once");
    }

    /// Two separate model messages, each delta + consolidation, must each be
    /// emitted once — the accumulator resets at the message boundary.
    #[tokio::test]
    async fn two_messages_each_emitted_once() {
        let text = text_of(vec![
            model_delta("one"),
            model_complete("one"),
            model_delta("two"),
            model_complete("two"),
        ])
        .await;
        assert_eq!(text, "onetwo");
    }

    #[test]
    fn dedup_model_text_skips_exact_consolidation() {
        let mut s = String::new();
        assert_eq!(
            dedup_model_text("abc".to_owned(), true, &mut s),
            Some("abc".to_owned())
        );
        assert_eq!(dedup_model_text("abc".to_owned(), false, &mut s), None);
    }

    #[test]
    fn dedup_model_text_trims_grown_snapshot() {
        let mut s = String::new();
        assert_eq!(
            dedup_model_text("ab".to_owned(), true, &mut s),
            Some("ab".to_owned())
        );
        assert_eq!(
            dedup_model_text("abcd".to_owned(), false, &mut s),
            Some("cd".to_owned())
        );
    }

    #[test]
    fn dedup_model_text_non_streaming_emits_content() {
        let mut s = String::new();
        assert_eq!(
            dedup_model_text("full".to_owned(), false, &mut s),
            Some("full".to_owned())
        );
    }

    // ── Error detection tests ────────────────────────────────────────

    #[test]
    fn error_status_is_detected() {
        let step = step_with(StepStatus::Error, "", "some content");
        assert_eq!(step.status, StepStatus::Error);
        assert!(step.error.is_empty());
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
    fn normal_step_not_treated_as_error() {
        let step = step_with(StepStatus::Done, "", "normal content");
        let has_error_status = step.status == StepStatus::Error;
        let has_error_field = !step.error.is_empty();
        assert!(!has_error_status && !has_error_field);
    }

    #[test]
    fn empty_content_with_done_status_is_not_error() {
        let step = step_with(StepStatus::Done, "", "");
        let has_error_status = step.status == StepStatus::Error;
        let has_error_field = !step.error.is_empty();
        assert!(!has_error_status && !has_error_field);
    }

    // ── format_error_message tests ──────────────────────────────────

    #[test]
    fn format_uses_error_field_when_present() {
        let step = step_with(StepStatus::Error, "quota exceeded", "some content");
        assert_eq!(format_error_message(&step), "quota exceeded");
    }

    #[test]
    fn format_falls_back_to_content_when_no_error_field() {
        let step = step_with(StepStatus::Error, "", "agent terminated");
        let msg = format_error_message(&step);
        assert!(msg.contains("agent terminated"), "got: {msg}");
        assert!(msg.contains("Error"), "got: {msg}");
    }

    #[test]
    fn format_uses_content_delta_when_content_empty() {
        let step = Step {
            status: StepStatus::Error,
            content_delta: "delta error text".to_string(),
            ..Step::default()
        };
        let msg = format_error_message(&step);
        assert!(msg.contains("delta error text"), "got: {msg}");
    }

    // ── output_after_error state machine tests ──────────────────────
    //
    // These verify the tracking logic used by `stream_steps_to_writer`
    // to decide whether to propagate errors at end-of-stream.

    /// Simulates the state machine from `stream_steps_to_writer`.
    /// Returns (`last_error`, `output_after_error`) after processing events.
    fn simulate_stream(events: &[super::StepContent]) -> (Option<String>, bool) {
        // Delegate to the real state machine so these tests exercise production
        // logic rather than a parallel copy. (The early-stop signal is covered
        // separately in the `consecutive model-error` tests below.)
        let mut state = super::StreamErrorState::new(super::StreamLimits::default());
        for event in events {
            state.observe(event);
        }
        (state.last_error, state.output_after_error)
    }

    #[test]
    fn error_only_propagates() {
        let (last_error, output_after) =
            simulate_stream(&[super::StepContent::Error("503 unavailable".into())]);
        assert!(last_error.is_some());
        assert!(!output_after, "No output after error → should propagate");
    }

    #[test]
    fn error_then_output_is_recovered() {
        let (last_error, output_after) = simulate_stream(&[
            super::StepContent::Error("model output empty".into()),
            super::StepContent::Output,
        ]);
        assert!(last_error.is_some());
        assert!(
            output_after,
            "Output after error → recovered, don't propagate"
        );
    }

    #[test]
    fn error_then_output_then_error_propagates() {
        let (last_error, output_after) = simulate_stream(&[
            super::StepContent::Error("first error".into()),
            super::StepContent::Output,
            super::StepContent::Error("second error".into()),
        ]);
        assert_eq!(last_error.as_deref(), Some("second error"));
        assert!(!output_after, "Last error had no output after → propagate");
    }

    #[test]
    fn clean_stream_no_error() {
        let (last_error, output_after) = simulate_stream(&[
            super::StepContent::Output,
            super::StepContent::Empty,
            super::StepContent::Output,
        ]);
        assert!(last_error.is_none());
        assert!(!output_after);
    }

    #[test]
    fn output_before_error_does_not_count_as_recovery() {
        let (last_error, output_after) = simulate_stream(&[
            super::StepContent::Output, // output BEFORE error
            super::StepContent::Error("late error".into()),
        ]);
        assert!(last_error.is_some());
        assert!(
            !output_after,
            "Output before (not after) error → should propagate"
        );
    }

    #[test]
    fn empty_steps_do_not_affect_recovery() {
        let (last_error, output_after) = simulate_stream(&[
            super::StepContent::Error("error".into()),
            super::StepContent::Empty,
            super::StepContent::Empty,
        ]);
        assert!(last_error.is_some());
        assert!(!output_after, "Empty steps don't count as recovery");
    }

    #[test]
    fn multiple_errors_then_output_is_recovered() {
        let (last_error, output_after) = simulate_stream(&[
            super::StepContent::Error("first".into()),
            super::StepContent::Error("second".into()),
            super::StepContent::Output,
        ]);
        assert_eq!(last_error.as_deref(), Some("second"));
        assert!(output_after, "Output after last error → recovered");
    }

    // ── consecutive model-error early-stop tests ────────────────────

    fn model_error() -> super::StepContent {
        super::StepContent::Error(
            "model output must contain either output text or tool calls".into(),
        )
    }

    #[test]
    fn three_consecutive_model_errors_stop_the_stream() {
        let mut state = super::StreamErrorState::new(super::StreamLimits::default());
        assert!(
            !state.observe(&model_error()),
            "1st model error keeps streaming"
        );
        assert!(
            !state.observe(&model_error()),
            "2nd model error keeps streaming"
        );
        assert!(
            state.observe(&model_error()),
            "3rd consecutive model error must stop the stream"
        );
        assert!(state.last_error.is_some());
        assert!(
            !state.output_after_error,
            "no output followed → the error must propagate"
        );
    }

    #[test]
    fn output_resets_the_model_error_streak() {
        let mut state = super::StreamErrorState::new(super::StreamLimits::default());
        assert!(!state.observe(&model_error()));
        assert!(!state.observe(&model_error()));
        // A usable step resets the streak...
        assert!(!state.observe(&super::StepContent::Output));
        // ...so two further model errors still do not trip the limit.
        assert!(!state.observe(&model_error()));
        assert!(!state.observe(&model_error()));
        assert_eq!(state.consecutive_model_errors, 2);
    }

    #[test]
    fn transient_errors_do_not_count_toward_the_model_limit() {
        let mut state = super::StreamErrorState::new(super::StreamLimits::default());
        // The SDK's own backoff handles transport/API errors; they must never
        // trip the model-quality early-stop.
        for _ in 0..5 {
            assert!(!state.observe(&super::StepContent::Error("503 unavailable".into())));
        }
        assert_eq!(state.consecutive_model_errors, 0);
        assert!(state.last_error.is_some());
    }

    #[test]
    fn a_transient_error_resets_the_model_error_streak() {
        let mut state = super::StreamErrorState::new(super::StreamLimits::default());
        assert!(!state.observe(&model_error()));
        assert!(!state.observe(&model_error()));
        // A different (non-model) error breaks the consecutive model streak.
        assert!(!state.observe(&super::StepContent::Error("503 unavailable".into())));
        assert_eq!(state.consecutive_model_errors, 0);
    }

    #[test]
    fn empty_steps_do_not_reset_the_model_error_streak() {
        let mut state = super::StreamErrorState::new(super::StreamLimits::default());
        assert!(!state.observe(&model_error()));
        assert!(!state.observe(&super::StepContent::Empty));
        // The empty (metadata-only) step neither counts nor resets, so the next
        // model error is the 2nd — not enough to stop yet.
        assert!(!state.observe(&model_error()));
        // The 3rd consecutive model error (ignoring the empty) trips the limit.
        assert!(state.observe(&model_error()));
    }

    // ── runaway thinking-only (empty step) early-stop tests ─────────

    #[test]
    fn runaway_thinking_only_stream_is_aborted() {
        let mut state = super::StreamErrorState::new(super::StreamLimits::default());
        let limit = super::DEFAULT_MAX_CONSECUTIVE_EMPTY_STEPS;
        // All but the last empty step keep the stream going.
        for i in 1..limit {
            assert!(
                !state.observe(&super::StepContent::Empty),
                "empty step {i} should not yet abort"
            );
        }
        // The step that reaches the limit aborts and records a synthetic,
        // model-quality-classified error so the orchestrator recovers.
        assert!(
            state.observe(&super::StepContent::Empty),
            "reaching the empty-step limit must abort the stream"
        );
        let err = state.last_error.expect("synthetic error recorded");
        assert!(
            super::is_model_quality_error(&err),
            "synthetic runaway error must be model-quality so it routes to recovery"
        );
        assert!(!state.output_after_error, "no output → error propagates");
    }

    #[test]
    fn output_resets_the_empty_step_streak() {
        let mut state = super::StreamErrorState::new(super::StreamLimits::default());
        // Stream thinking-only steps up to just below the limit...
        for _ in 0..(super::DEFAULT_MAX_CONSECUTIVE_EMPTY_STEPS - 1) {
            assert!(!state.observe(&super::StepContent::Empty));
        }
        // ...then a productive step resets the streak.
        assert!(!state.observe(&super::StepContent::Output));
        assert_eq!(state.consecutive_empty_steps, 0);
        // A fresh run of thinking-only steps starts over and does not abort.
        assert!(!state.observe(&super::StepContent::Empty));
    }

    #[test]
    fn interleaved_output_prevents_runaway_abort() {
        let mut state = super::StreamErrorState::new(super::StreamLimits::default());
        // A healthy long turn: many thinking steps punctuated by output never
        // reaches the empty-step ceiling.
        for _ in 0..10 {
            for _ in 0..(super::DEFAULT_MAX_CONSECUTIVE_EMPTY_STEPS - 1) {
                assert!(!state.observe(&super::StepContent::Empty));
            }
            assert!(!state.observe(&super::StepContent::Output));
        }
        assert!(state.last_error.is_none(), "healthy turn records no error");
    }

    // ── disabled-limit (zero = unlimited) tests ─────────────────────

    #[test]
    fn zero_model_error_limit_never_aborts() {
        let limits = super::StreamLimits {
            max_model_errors: 0,
            max_empty_steps: super::DEFAULT_MAX_CONSECUTIVE_EMPTY_STEPS,
            channel_buffer: crate::streaming::DEFAULT_CHANNEL_BUFFER,
        };
        let mut state = super::StreamErrorState::new(limits);
        // Far more model errors than the default limit — none should trip.
        for _ in 0..100 {
            assert!(
                !state.observe(&model_error()),
                "zero limit must never abort on model errors"
            );
        }
        // The errors are still recorded for end-of-stream propagation.
        assert!(state.last_error.is_some());
    }

    #[test]
    fn zero_empty_step_limit_never_aborts() {
        let limits = super::StreamLimits {
            max_model_errors: super::DEFAULT_MAX_CONSECUTIVE_MODEL_ERRORS,
            max_empty_steps: 0,
            channel_buffer: crate::streaming::DEFAULT_CHANNEL_BUFFER,
        };
        let mut state = super::StreamErrorState::new(limits);
        // Far more empty steps than the default limit — none should trip.
        for _ in 0..1000 {
            assert!(
                !state.observe(&super::StepContent::Empty),
                "zero limit must never abort on empty steps"
            );
        }
        assert!(state.last_error.is_none(), "no synthetic error recorded");
    }

    #[test]
    fn stream_limits_from_config_uses_overrides() {
        let config = super::super::config::RuntimeConfig {
            max_consecutive_model_errors: Some(10),
            max_consecutive_empty_steps: Some(42),
            ..Default::default()
        };
        let limits = super::StreamLimits::from_config(&config);
        assert_eq!(limits.max_model_errors, 10);
        assert_eq!(limits.max_empty_steps, 42);
    }

    #[test]
    fn stream_limits_from_config_uses_defaults_for_none() {
        let config = super::super::config::RuntimeConfig::default();
        let limits = super::StreamLimits::from_config(&config);
        assert_eq!(
            limits.max_model_errors,
            super::DEFAULT_MAX_CONSECUTIVE_MODEL_ERRORS
        );
        assert_eq!(
            limits.max_empty_steps,
            super::DEFAULT_MAX_CONSECUTIVE_EMPTY_STEPS
        );
    }
}

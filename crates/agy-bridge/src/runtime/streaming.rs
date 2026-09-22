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
    /// The step was an error (no content produced, error forwarded). Carries
    /// the harness-reported HTTP status code (`0` when unknown).
    Error { message: String, http_code: u16 },
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
    last_http_code: u16,
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
            StepContent::Error { message, http_code } => {
                // An error breaks any thinking-only streak.
                self.consecutive_empty_steps = 0;
                if is_model_quality_error(message) {
                    self.consecutive_model_errors += 1;
                } else {
                    // A transport/API error breaks the model-quality streak;
                    // the SDK's own retry policy governs those.
                    self.consecutive_model_errors = 0;
                }
                self.last_error = Some(message.clone());
                self.last_http_code = *http_code;
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
                    // Synthetic model-quality failure — no HTTP status applies.
                    self.last_http_code = crate::error::HTTP_CODE_UNKNOWN;
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
        return StepContent::Error {
            message: error_msg,
            http_code,
        };
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
    // Mirror the SDK's `receive_chunks`: text is part of the primary stream
    // only for the model's user-facing output (source == MODEL && target ==
    // USER). Text from other sources (user echoes, system) or targeted at
    // subagents/the environment is not yielded as a text chunk by the SDK, so
    // the bridge must not leak it into the text/chunk/event streams either.
    let is_model = step.source == crate::types::StepSource::Model;
    let is_target_user = step.target == crate::types::StepTarget::User;
    if !(is_model && is_target_user) {
        return;
    }

    let (raw, is_delta) = if step.content_delta.is_empty() {
        (std::mem::take(&mut step.content), false)
    } else {
        (std::mem::take(&mut step.content_delta), true)
    };
    if raw.is_empty() {
        return;
    }

    // De-duplicate a consolidated "complete response" step that repeats text
    // already streamed via deltas within the same message. The SDK yields
    // *both* the incremental deltas and a final full-content step per turn;
    // forwarding both would double the response text. Unlike the SDK's
    // delta-only `receive_chunks`, the bridge also surfaces the final full
    // content when a turn produced no deltas, so `.text()` is never empty for
    // non-streaming turns.
    let Some(text) = dedup_model_text(raw, is_delta, streamed_text) else {
        return;
    };

    crate::streaming::ChatResponseWriter::fan_out(
        &writer.subs.event,
        &writer.event_tx,
        crate::streaming::ResponseEvent::TextChunk(text.clone()),
        "event",
    )
    .await;
    crate::streaming::ChatResponseWriter::fan_out(
        &writer.subs.chunk,
        &writer.chunk_tx,
        crate::streaming::StreamChunk::Text(text.clone()),
        "chunk",
    )
    .await;
    crate::streaming::ChatResponseWriter::fan_out(&writer.subs.text, &writer.text_tx, text, "text")
        .await;
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
    // Mirror the SDK's `receive_chunks`: thoughts are streamed only for the
    // model's user-facing output (source == MODEL && target == USER). Thoughts
    // from other sources or targeted at subagents/the environment are not
    // yielded by the SDK, so the bridge must not leak them either.
    let is_model = step.source == crate::types::StepSource::Model;
    let is_target_user = step.target == crate::types::StepTarget::User;
    if !(is_model && is_target_user) {
        return;
    }

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

    let mut saw_any_step = false;
    loop {
        match process_next_step_iteration(aiter_py, agent_id).await {
            StepIterationResult::Step(step) => {
                saw_any_step = true;
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
                // Python exception (not a step) — always fatal. No structured
                // HTTP status is available, so classification falls back to the
                // message text.
                send_stream_error(writer, err_msg, crate::error::HTTP_CODE_UNKNOWN);
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
            send_stream_error(writer, error_msg, state.last_http_code);
        }
    } else if !saw_any_step {
        tracing::warn!(
            agent_id = ?agent_id,
            "Stream ended without yielding any steps — propagating error"
        );
        send_stream_error(
            writer,
            "Turn stream ended prematurely without yielding any steps".to_string(),
            crate::error::HTTP_CODE_UNKNOWN,
        );
    }
}

fn send_stream_error(
    writer: &crate::streaming::ChatResponseWriter,
    message: String,
    http_code: u16,
) {
    // Use try_send to avoid deadlock: the error channel has capacity 1.
    // The writer must be dropped to close the text channel (which
    // handle.text() is waiting on). If error_tx is already full, the
    // first error wins; subsequent errors are logged but not queued.
    if let Err(e) = writer
        .error_tx
        .try_send(crate::streaming::StreamError::with_http_code(
            message, http_code,
        ))
    {
        tracing::debug!("Error channel full or closed (first error wins): {e}");
    }
}

#[cfg(test)]
#[path = "streaming_tests.rs"]
mod tests;

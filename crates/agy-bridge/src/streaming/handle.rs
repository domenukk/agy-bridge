//! The receiving/reading side of the streaming channel pair.

use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex, atomic::Ordering},
};

use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use super::types::{
    ChatResponseSharedState, ChatResult, ERROR_DRAIN_TIMEOUT, ResponseEvent, StreamChunk,
    StreamError, StreamReceivers, StreamSubscriptions, ToolCallEvent,
};
use crate::types::{Step, StopReason, UsageMetadata};

type CancelCallback = Box<
    dyn FnOnce() -> Pin<Box<dyn Future<Output = Result<(), crate::error::Error>> + Send>> + Send,
>;

/// Handle to a streaming chat response.
///
/// Created by [`AgentHandle::chat()`](crate::agent::AgentHandle::chat). Provides
/// independent channels for text tokens, thinking tokens, and tool-call events.
///
/// Each stream accessor can only be called once — subsequent calls return `None`
/// because the underlying receiver has already been taken.
pub struct ChatResponseHandle {
    /// All per-stream receivers, grouped for clarity.
    pub(super) rx: StreamReceivers,
    /// Per-view subscription flags, shared with the writer.
    ///
    /// Set when the consumer attaches to a stream so the writer only fans out
    /// to channels that are actually being drained.
    pub(super) subs: Arc<StreamSubscriptions>,
    /// Token usage metadata, populated after the stream completes.
    pub(super) usage: Option<UsageMetadata>,
    /// Structured output from a `response_schema`-configured agent.
    pub(super) structured_output_value: Option<serde_json::Value>,
    /// Trajectory stop reason populated when the turn finishes.
    pub(super) stop_reason: StopReason,
    /// Optional runtime cancellation callback wired by [`AgentHandle`](crate::agent::AgentHandle).
    pub(super) cancel_fn: Option<CancelCallback>,
    /// Weak liveness reference to the paired [`ChatResponseWriter`](super::writer::ChatResponseWriter).
    pub(super) writer_alive: std::sync::Weak<()>,
    /// Shared state to receive metadata updates from the python bridge thread.
    pub(crate) shared_state: Arc<Mutex<ChatResponseSharedState>>,
}

impl std::fmt::Debug for ChatResponseHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChatResponseHandle")
            .field("rx", &self.rx)
            .field("subs", &self.subs)
            .field("usage", &self.usage)
            .field("structured_output_value", &self.structured_output_value)
            .field("stop_reason", &self.stop_reason)
            .field("has_canceller", &self.cancel_fn.is_some())
            .field("writer_alive", &(self.writer_alive.strong_count() > 0))
            .field("shared_state", &self.shared_state)
            .finish()
    }
}

impl ChatResponseHandle {
    /// Take the text token receiver for token-by-token streaming.
    ///
    /// Returns `None` if the receiver was already taken.
    pub fn take_text_stream(&mut self) -> Option<mpsc::Receiver<String>> {
        self.subs.text.store(true, Ordering::Release);
        self.rx.text.take()
    }

    /// Take the thinking token receiver.
    ///
    /// Returns `None` if the receiver was already taken.
    pub fn take_thought_stream(&mut self) -> Option<mpsc::Receiver<String>> {
        self.subs.thought.store(true, Ordering::Release);
        self.rx.thought.take()
    }

    /// Take the tool call event receiver.
    ///
    /// Returns `None` if the receiver was already taken.
    pub fn take_tool_call_stream(&mut self) -> Option<mpsc::Receiver<ToolCallEvent>> {
        self.subs.tool_call.store(true, Ordering::Release);
        self.rx.tool_call.take()
    }

    /// Take the raw step receiver.
    ///
    /// Returns `None` if the receiver was already taken.
    /// Prefer [`receive_steps()`](Self::receive_steps) for `StreamExt`-compatible usage.
    pub fn take_step_stream(&mut self) -> Option<mpsc::Receiver<Step>> {
        self.subs.step.store(true, Ordering::Release);
        self.rx.step.take()
    }

    /// Take the step stream for consuming with `StreamExt::next()`.
    ///
    /// Returns `None` if the stream was already taken.
    ///
    /// # Example
    ///
    /// ```
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// use agy_bridge::streaming;
    /// use tokio_stream::StreamExt;
    ///
    /// let (_writer, mut handle) = streaming::channel();
    /// drop(_writer); // close the channel so the stream ends
    /// let mut steps = handle.receive_steps().unwrap();
    /// while let Some(step) = steps.next().await {
    ///     println!("step: {:?}", step.step_type);
    /// }
    /// # });
    pub fn receive_steps(&mut self) -> Option<impl tokio_stream::Stream<Item = Step> + use<>> {
        self.subs.step.store(true, Ordering::Release);
        self.rx.step.take().map(ReceiverStream::new)
    }

    /// Take the unified chunk stream for consuming with `StreamExt::next()`.
    ///
    /// Returns `None` if the stream was already taken.
    ///
    /// # Example
    ///
    /// ```
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// use agy_bridge::streaming::{self, StreamChunk};
    /// use tokio_stream::StreamExt;
    ///
    /// let (_writer, mut handle) = streaming::channel();
    /// drop(_writer); // close the channel so the stream ends
    /// let mut chunks = handle.receive_chunks().unwrap();
    /// while let Some(chunk) = chunks.next().await {
    ///     match chunk {
    ///         StreamChunk::Text(t) => print!("{t}"),
    ///         StreamChunk::Thought(t) => eprintln!("thought: {t}"),
    ///         StreamChunk::ToolCall(tc) => eprintln!("tool: {}", tc.name),
    ///         _ => {}
    ///     }
    /// }
    /// # });
    pub fn receive_chunks(
        &mut self,
    ) -> Option<impl tokio_stream::Stream<Item = StreamChunk> + use<>> {
        self.subs.chunk.store(true, Ordering::Release);
        self.rx.chunk.take().map(ReceiverStream::new)
    }

    /// Take the unified chunk receiver as a plain `mpsc::Receiver`.
    ///
    /// Returns `None` if the receiver was already taken. Prefer
    /// [`receive_chunks()`](Self::receive_chunks) for `StreamExt`-compatible
    /// usage; this variant exists for consumers that drain the channel with
    /// `recv()`.
    pub fn take_chunk_stream(&mut self) -> Option<mpsc::Receiver<StreamChunk>> {
        self.subs.chunk.store(true, Ordering::Release);
        self.rx.chunk.take()
    }

    /// Take the timeline event receiver.
    ///
    /// Returns `None` if the receiver was already taken. Prefer
    /// [`resolve()`](Self::resolve) if you want the ordered event timeline;
    /// this variant lets a consumer drain `event_tx` incrementally with
    /// `recv()`.
    ///
    /// # Note
    ///
    /// Taking this stream *subscribes* to the event timeline, so the writer
    /// will fan every step out to `event_tx`. As with any subscribed stream,
    /// drain it concurrently (e.g. alongside [`text()`](Self::text)) so it
    /// keeps up. Views you never take are simply skipped by the writer and can
    /// never stall the stream.
    pub fn take_event_stream(&mut self) -> Option<mpsc::Receiver<ResponseEvent>> {
        self.subs.event.store(true, Ordering::Release);
        self.rx.event.take()
    }

    /// Drain the text stream and return the complete response text.
    ///
    /// Consumes the handle — use the `take_*` methods instead if you need
    /// to keep streaming individual channels.
    ///
    /// # Errors
    ///
    /// Returns a [`StreamError`] if the Python side reported an error.
    pub async fn text(mut self) -> Result<ChatResult, StreamError> {
        self.subs.text.store(true, Ordering::Release);
        let mut buf = String::new();

        if let Some(mut rx) = self.rx.text.take() {
            while let Some(token) = rx.recv().await {
                buf.push_str(&token);
            }
        }

        // Check for errors. Use a brief timeout rather than try_recv() to
        // catch errors that are sent just after the text channel closes.
        if let Some(mut err_rx) = self.rx.error.take()
            && let Ok(Some(err)) = tokio::time::timeout(ERROR_DRAIN_TIMEOUT, err_rx.recv()).await
        {
            match self.shared_state.lock() {
                Ok(mut state) => {
                    state.structured_output = None;
                    state.turn_failed = true;
                }
                Err(poisoned) => {
                    let mut state = poisoned.into_inner();
                    state.structured_output = None;
                    state.turn_failed = true;
                }
            }
            return Err(err);
        }

        self.finalize();

        Ok(ChatResult {
            text: buf,
            usage: self.usage,
            structured_output: self.structured_output_value,
            stop_reason: self.stop_reason,
        })
    }

    /// Attach a turn-cancellation callback invoked by [`cancel()`](Self::cancel).
    pub(crate) fn attach_canceller(&mut self, callback: CancelCallback) {
        self.cancel_fn = Some(callback);
    }

    /// Return a weak reference to the paired writer's liveness token (`strong_count() == 0` once completed).
    pub(crate) fn writer_alive_weak(&self) -> std::sync::Weak<()> {
        self.writer_alive.clone()
    }

    /// Cancel the active turn associated with this response handle (`0.1.24`+).
    ///
    /// # Errors
    ///
    /// Returns [`Error`](crate::error::Error) if forwarding the halt signal to the runtime fails.
    pub async fn cancel(&mut self) -> Result<(), crate::error::Error> {
        if let Some(cb) = self.cancel_fn.take() {
            cb().await?;
        }
        Ok(())
    }

    /// Finalize the response handle by pulling usage, stop reason, and structured output
    /// from the shared state. Called after the stream has been fully drained.
    pub fn finalize(&mut self) {
        match self.shared_state.lock() {
            Ok(state) => {
                self.usage = state.usage.clone();
                self.stop_reason = state.stop_reason;
                self.structured_output_value = if state.turn_failed {
                    None
                } else {
                    state.structured_output.clone()
                };
            }
            Err(poisoned) => {
                tracing::warn!(
                    "ChatResponseHandle shared_state mutex was poisoned during finalize, recovering"
                );
                let state = poisoned.into_inner();
                self.usage = state.usage.clone();
                self.stop_reason = state.stop_reason;
                self.structured_output_value = if state.turn_failed {
                    None
                } else {
                    state.structured_output.clone()
                };
            }
        }
    }

    /// Return the structured output, if available and the turn succeeded.
    ///
    /// Only populated when the agent was configured with a `response_schema`
    /// and the model returned a valid JSON payload without turn failure.
    #[must_use]
    pub const fn structured_output(&self) -> Option<&serde_json::Value> {
        self.structured_output_value.as_ref()
    }

    /// Check for any pending stream error, finalize metadata, and return the turn's
    /// structured output (`0.1.24`+).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Stream`](crate::error::Error::Stream) if an error arrived on the stream.
    pub fn try_structured_output(
        &mut self,
    ) -> Result<Option<serde_json::Value>, crate::error::Error> {
        if let Some(ref mut err_rx) = self.rx.error {
            match err_rx.try_recv() {
                Ok(err) => {
                    match self.shared_state.lock() {
                        Ok(mut state) => {
                            state.structured_output = None;
                            state.turn_failed = true;
                            if state.last_stream_error.is_none() {
                                state.last_stream_error = Some(err.clone());
                            }
                        }
                        Err(poisoned) => {
                            let mut state = poisoned.into_inner();
                            state.structured_output = None;
                            state.turn_failed = true;
                            if state.last_stream_error.is_none() {
                                state.last_stream_error = Some(err.clone());
                            }
                        }
                    }
                    self.structured_output_value = None;
                    return Err(crate::error::Error::Stream(err));
                }
                Err(mpsc::error::TryRecvError::Empty | mpsc::error::TryRecvError::Disconnected) => {
                }
            }
        }
        let recorded_err = match self.shared_state.lock() {
            Ok(state) => state.last_stream_error.clone(),
            Err(poisoned) => poisoned.into_inner().last_stream_error.clone(),
        };
        if let Some(err) = recorded_err {
            self.structured_output_value = None;
            return Err(crate::error::Error::Stream(err));
        }
        self.finalize();
        Ok(self.structured_output_value.clone())
    }

    /// Return the trajectory stop reason for this turn (`StopReason::Unspecified` on normal completion).
    #[must_use]
    pub fn stop_reason(&self) -> StopReason {
        if self.stop_reason != StopReason::Unspecified {
            return self.stop_reason;
        }
        match self.shared_state.lock() {
            Ok(state) => state.stop_reason,
            Err(poisoned) => poisoned.into_inner().stop_reason,
        }
    }

    /// Return the token usage metadata, if available.
    ///
    /// Populated after [`finalize()`](Self::finalize) or [`text()`](Self::text).
    #[must_use]
    pub const fn usage_metadata(&self) -> Option<&UsageMetadata> {
        self.usage.as_ref()
    }

    /// Return a reference-counted handle to the shared state.
    ///
    /// This allows callers to clone the `Arc` **before** consuming the handle
    /// via [`text()`](Self::text) or [`resolve()`](Self::resolve), and then
    /// read usage metadata / structured output from the shared state
    /// afterwards.
    #[doc(hidden)]
    #[must_use]
    pub fn shared_state(&self) -> Arc<Mutex<ChatResponseSharedState>> {
        Arc::clone(&self.shared_state)
    }

    /// Drain all events and return them as an ordered timeline.
    ///
    /// Consumes the handle — use the `take_*` methods instead if you need
    /// to keep streaming individual channels.
    ///
    /// # Errors
    ///
    /// Returns a [`StreamError`] if the backend reported an error during the turn.
    pub async fn resolve(mut self) -> Result<Vec<ResponseEvent>, StreamError> {
        self.subs.event.store(true, Ordering::Release);
        let mut events = Vec::new();
        if let Some(mut rx) = self.rx.event.take() {
            while let Some(event) = rx.recv().await {
                events.push(event);
            }
        }
        if let Some(mut err_rx) = self.rx.error.take()
            && let Ok(Some(err)) = tokio::time::timeout(ERROR_DRAIN_TIMEOUT, err_rx.recv()).await
        {
            match self.shared_state.lock() {
                Ok(mut state) => {
                    state.structured_output = None;
                    state.turn_failed = true;
                }
                Err(poisoned) => {
                    let mut state = poisoned.into_inner();
                    state.structured_output = None;
                    state.turn_failed = true;
                }
            }
            return Err(err);
        }
        self.finalize();
        Ok(events)
    }
}

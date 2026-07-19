//! The receiving/reading side of the streaming channel pair.

use std::sync::{Arc, Mutex, atomic::Ordering};

use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use super::types::{
    ChatResponseSharedState, ChatResult, ERROR_DRAIN_TIMEOUT, ResponseEvent, StreamChunk,
    StreamError, StreamReceivers, StreamSubscriptions, ToolCallEvent,
};
use crate::types::{Step, UsageMetadata};

/// Handle to a streaming chat response.
///
/// Created by [`AgentHandle::chat()`](crate::agent::AgentHandle::chat). Provides
/// independent channels for text tokens, thinking tokens, and tool-call events.
///
/// Each stream accessor can only be called once — subsequent calls return `None`
/// because the underlying receiver has already been taken.
#[derive(Debug)]
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
    /// Shared state to receive metadata updates from the python bridge thread.
    pub(crate) shared_state: Arc<Mutex<ChatResponseSharedState>>,
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
    pub fn receive_steps(&mut self) -> Option<impl tokio_stream::Stream<Item = Step>> {
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
    pub fn receive_chunks(&mut self) -> Option<impl tokio_stream::Stream<Item = StreamChunk>> {
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
            return Err(err);
        }

        self.finalize();

        Ok(ChatResult {
            text: buf,
            usage: self.usage,
            structured_output: self.structured_output_value,
        })
    }

    /// Finalize the response handle by pulling usage and structured output
    /// from the shared state. Called after the stream has been fully drained.
    pub fn finalize(&mut self) {
        // NOLINT: Mutex::lock only fails if poisoned; else branch logs tracing::error!
        if let Ok(state) = self.shared_state.lock() {
            self.usage = state.usage.clone();
            self.structured_output_value = state.structured_output.clone();
        } else {
            tracing::error!(
                "ChatResponseHandle shared_state mutex poisoned during finalize — \
                 usage and structured_output will be unavailable"
            );
        }
    }

    /// Return the structured output, if available.
    ///
    /// Only populated when the agent was configured with a `response_schema`
    /// and the model returned a valid JSON payload.
    #[must_use]
    pub const fn structured_output(&self) -> Option<&serde_json::Value> {
        self.structured_output_value.as_ref()
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
    pub async fn resolve(mut self) -> Vec<ResponseEvent> {
        self.subs.event.store(true, Ordering::Release);
        let mut events = Vec::new();
        if let Some(mut rx) = self.rx.event.take() {
            while let Some(event) = rx.recv().await {
                events.push(event);
            }
        }
        self.finalize();
        events
    }
}

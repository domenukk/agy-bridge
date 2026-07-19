//! The sending/writing side of the streaming channel pair.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use tokio::sync::mpsc;

use super::types::{
    ChatResponseSharedState, ResponseEvent, StreamChunk, StreamError, StreamSubscriptions,
    ToolCallEvent,
};
use crate::types::Step;

/// Error returned when sending to a [`ChatResponseWriter`] channel fails.
///
/// This wraps the underlying channel error to avoid leaking the
/// `tokio::sync::mpsc::error::SendError<T>` generic into the public API.
///
/// # Example
///
/// ```
/// use agy_bridge::streaming::WriterError;
///
/// let err = WriterError::new("receiver dropped");
/// assert_eq!(err.to_string(), "receiver dropped");
/// ```
#[derive(Debug)]
pub struct WriterError {
    /// Human-readable description of the failure.
    pub message: String,
}

impl WriterError {
    /// Create a new writer error.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for WriterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for WriterError {}

impl<T> From<mpsc::error::SendError<T>> for WriterError {
    fn from(err: mpsc::error::SendError<T>) -> Self {
        Self {
            message: format!("channel send failed: {err}"),
        }
    }
}

/// The sending side of a [`ChatResponseHandle`](super::handle::ChatResponseHandle),
/// held by the Python bridge thread that drives the SDK's async iterator.
pub struct ChatResponseWriter {
    /// Sends text tokens.
    pub(crate) text_tx: mpsc::Sender<String>,
    /// Sends thinking tokens.
    pub(crate) thought_tx: mpsc::Sender<String>,
    /// Sends tool call events.
    pub(crate) tool_call_tx: mpsc::Sender<ToolCallEvent>,
    /// Sends a stream error (at most one).
    pub(crate) error_tx: mpsc::Sender<StreamError>,
    /// Sends ordered [`ResponseEvent`]s for the resolve timeline.
    pub(crate) event_tx: mpsc::Sender<ResponseEvent>,
    /// Sends [`Step`] objects as they are produced.
    ///
    /// The sender must be held to keep the channel alive for
    /// [`ChatResponseHandle::take_step_stream()`](super::handle::ChatResponseHandle::take_step_stream).
    /// It will be actively written once step-level streaming is wired through
    /// the command loop.
    pub(crate) step_tx: mpsc::Sender<Step>,
    /// Sends unified [`StreamChunk`]s.
    pub(crate) chunk_tx: mpsc::Sender<StreamChunk>,
    /// Per-view subscription flags, shared with the handle.
    ///
    /// The writer consults these before fanning out so it never sends to a
    /// channel the consumer isn't draining (which would block it forever).
    pub(crate) subs: Arc<StreamSubscriptions>,
    /// Shared state to send metadata updates back to the handle.
    pub(crate) shared_state: Arc<Mutex<ChatResponseSharedState>>,
}

impl ChatResponseWriter {
    /// Fan a streamed item out to a single optional "view" channel.
    ///
    /// This is the deadlock-safe primitive the bridge uses for its multi-channel
    /// fan-out. It:
    ///
    /// * **Skips** the send entirely when no consumer has subscribed to this
    ///   view. Its receiver is never drained, so a real send would fill the
    ///   bounded buffer and block the writer forever — silently stalling the
    ///   whole stream. This is the root cause of the "no progress" hangs.
    /// * Treats a **dropped receiver** mid-stream as an implicit unsubscribe:
    ///   it clears the flag and returns, so subsequent items skip immediately.
    ///
    /// A single view channel must never block or abort the entire stream.
    pub(crate) async fn fan_out<T>(
        subscribed: &AtomicBool,
        tx: &mpsc::Sender<T>,
        item: T,
        channel: &'static str,
    ) {
        if !subscribed.load(Ordering::Acquire) {
            return;
        }
        if let Err(e) = tx.send(item).await {
            subscribed.store(false, Ordering::Release);
            tracing::debug!(channel, error = %e, "fan-out receiver dropped; unsubscribing view");
        }
    }

    /// Send a text token.
    ///
    /// # Errors
    ///
    /// Returns [`WriterError`] if the receiver has been dropped.
    pub async fn send_text(&self, text: String) -> Result<(), WriterError> {
        self.text_tx.send(text).await.map_err(WriterError::from)
    }

    /// Send a thinking token.
    ///
    /// # Errors
    ///
    /// Returns [`WriterError`] if the receiver has been dropped.
    pub async fn send_thought(&self, thought: String) -> Result<(), WriterError> {
        self.thought_tx
            .send(thought)
            .await
            .map_err(WriterError::from)
    }

    /// Send a tool call event.
    ///
    /// # Errors
    ///
    /// Returns [`WriterError`] if the receiver has been dropped.
    pub async fn send_tool_call(&self, event: ToolCallEvent) -> Result<(), WriterError> {
        self.tool_call_tx
            .send(event)
            .await
            .map_err(WriterError::from)
    }

    /// Send an error.
    ///
    /// # Errors
    ///
    /// Returns [`WriterError`] if the receiver has been dropped.
    pub async fn send_error(&self, error: StreamError) -> Result<(), WriterError> {
        self.error_tx.send(error).await.map_err(WriterError::from)
    }

    /// Send a response event.
    ///
    /// # Errors
    ///
    /// Returns [`WriterError`] if the receiver has been dropped.
    pub async fn send_event(&self, event: ResponseEvent) -> Result<(), WriterError> {
        self.event_tx.send(event).await.map_err(WriterError::from)
    }

    /// Send a step.
    ///
    /// # Errors
    ///
    /// Returns [`WriterError`] if the receiver has been dropped.
    pub async fn send_step(&self, step: crate::types::Step) -> Result<(), WriterError> {
        self.step_tx.send(step).await.map_err(WriterError::from)
    }

    /// Send a unified stream chunk.
    ///
    /// # Errors
    ///
    /// Returns [`WriterError`] if the receiver has been dropped.
    pub async fn send_chunk(&self, chunk: StreamChunk) -> Result<(), WriterError> {
        self.chunk_tx.send(chunk).await.map_err(WriterError::from)
    }

    /// Store usage metadata in the shared state so the handle can read it
    /// after the stream completes.
    pub fn set_usage(&self, usage: crate::types::UsageMetadata) {
        match self.shared_state.lock() {
            Ok(mut state) => {
                state.usage = Some(usage);
            }
            Err(e) => {
                tracing::error!(
                    error = %e,
                    "ChatResponseWriter shared_state mutex poisoned in set_usage"
                );
            }
        }
    }

    /// Store structured output in the shared state so the handle can read it
    /// after the stream completes.
    pub fn set_structured_output(&self, value: serde_json::Value) {
        match self.shared_state.lock() {
            Ok(mut state) => {
                state.structured_output = Some(value);
            }
            Err(e) => {
                tracing::error!(
                    error = %e,
                    "ChatResponseWriter shared_state mutex poisoned in set_structured_output"
                );
            }
        }
    }
}

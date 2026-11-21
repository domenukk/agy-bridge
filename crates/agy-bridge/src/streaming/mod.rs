//! Streaming response bridge for the Antigravity SDK.
//!
//! Bridges the SDK's `ChatResponse` (Python async iterator) to tokio channels
//! so Rust consumers can stream text tokens, thinking tokens, and tool call
//! events independently.

mod handle;
mod types;
mod writer;

use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

use self::types::{StreamReceivers, StreamSubscriptions};
pub use self::{
    handle::ChatResponseHandle,
    types::{
        ChatResponseSharedState, ChatResult, ResponseEvent, StreamChunk, StreamError, ToolCallEvent,
    },
    writer::{ChatResponseWriter, WriterError},
};

/// Default channel buffer size. Large enough to avoid backpressure during
/// normal operation while bounding memory usage.
pub(crate) const DEFAULT_CHANNEL_BUFFER: usize = 256;

/// Create a paired `(ChatResponseWriter, ChatResponseHandle)`.
///
/// The writer is handed to the Python bridge thread; the handle is returned
/// to the Rust caller.
#[must_use]
pub fn channel() -> (ChatResponseWriter, ChatResponseHandle) {
    channel_with_buffer(DEFAULT_CHANNEL_BUFFER)
}

/// Create a paired `(ChatResponseWriter, ChatResponseHandle)` with a custom
/// channel buffer size (clamped to `>= 1` so zero can never panic).
///
/// Prefer [`channel()`] for default buffer sizing. Use this when you need
/// to tune memory usage or backpressure behavior.
#[must_use]
pub fn channel_with_buffer(buffer: usize) -> (ChatResponseWriter, ChatResponseHandle) {
    let buffer = buffer.max(1);
    let (text_tx, text_rx) = mpsc::channel(buffer);
    let (thought_tx, thought_rx) = mpsc::channel(buffer);
    let (tool_call_tx, tool_call_rx) = mpsc::channel(buffer);
    let (error_tx, error_rx) = mpsc::channel(1);
    let (event_tx, event_rx) = mpsc::channel(buffer);
    let (step_tx, step_rx) = mpsc::channel(buffer);
    let (chunk_tx, chunk_rx) = mpsc::channel(buffer);

    let shared_state = Arc::new(Mutex::new(ChatResponseSharedState::default()));
    let subs = Arc::new(StreamSubscriptions::default());
    let alive_token = Arc::new(());
    let writer_alive = Arc::downgrade(&alive_token);

    let writer = ChatResponseWriter {
        text_tx,
        thought_tx,
        tool_call_tx,
        error_tx,
        event_tx,
        step_tx,
        chunk_tx,
        subs: Arc::clone(&subs),
        shared_state: Arc::clone(&shared_state),
        _alive_token: alive_token,
    };

    let handle = ChatResponseHandle {
        rx: StreamReceivers::new(
            text_rx,
            thought_rx,
            tool_call_rx,
            error_rx,
            event_rx,
            step_rx,
            chunk_rx,
        ),
        subs,
        usage: None,
        structured_output_value: None,
        stop_reason: crate::types::StopReason::Unspecified,
        cancel_fn: None,
        writer_alive,
        shared_state,
    };

    (writer, handle)
}

#[cfg(test)]
mod tests;

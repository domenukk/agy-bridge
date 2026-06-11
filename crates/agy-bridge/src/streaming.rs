//! Streaming response bridge for the Antigravity SDK.
//!
//! Bridges the SDK's `ChatResponse` (Python async iterator) to tokio channels
//! so Rust consumers can stream text tokens, thinking tokens, and tool call
//! events independently.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use crate::types::{Step, UsageMetadata};

/// The result of draining a chat response via [`ChatResponseHandle::text()`].
///
/// Carries the full response text alongside optional metadata (token usage,
/// structured output). Dereferences to `&str` for ergonomic use:
///
/// ```rust
/// # #[tokio::main]
/// # async fn main() -> Result<(), agy_bridge::error::Error> {
/// # agy_bridge::load_dotenv();
/// # let bridge = agy_bridge::AgyBridge::builder().build()?;
/// # let agent = bridge.agent(
/// #     agy_bridge::config::AgentConfig::builder()
/// #         .system_instructions("Reply with 'Hello!' and nothing else. Never use tools.")
/// #         .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
/// #         .build()
/// # ).await?;
/// let result = agent
///     .chat("Reply with 'Hello!' and nothing else.")
///     .await?
///     .text()
///     .await?;
/// println!("{result}"); // prints text
/// if let Some(usage) = result.usage() { /* access metadata */ }
/// # agent.shutdown().await?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct ChatResult {
    text: String,
    usage: Option<UsageMetadata>,
    structured_output: Option<serde_json::Value>,
}

impl ChatResult {
    /// The full response text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Consume the result and return the inner `String`.
    #[must_use]
    pub fn into_string(self) -> String {
        self.text
    }

    /// Token usage metadata, if available.
    #[must_use]
    pub fn usage(&self) -> Option<&UsageMetadata> {
        self.usage.as_ref()
    }

    /// Structured output (JSON), if the agent was configured with a
    /// `response_schema` and the model returned valid JSON.
    #[must_use]
    pub fn structured_output(&self) -> Option<&serde_json::Value> {
        self.structured_output.as_ref()
    }
}

impl std::ops::Deref for ChatResult {
    type Target = str;
    fn deref(&self) -> &str {
        &self.text
    }
}

impl PartialEq<&str> for ChatResult {
    fn eq(&self, other: &&str) -> bool {
        self.text == *other
    }
}

impl PartialEq<String> for ChatResult {
    fn eq(&self, other: &String) -> bool {
        self.text == *other
    }
}

impl std::fmt::Display for ChatResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

impl From<ChatResult> for String {
    fn from(result: ChatResult) -> Self {
        result.text
    }
}

/// Brief timeout used when draining the error channel after the text stream
/// closes. Shared with [`crate::interactive`].
pub(crate) const ERROR_DRAIN_TIMEOUT: Duration = Duration::from_millis(50);

/// A tool call event received during streaming.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallEvent {
    /// Tool name (e.g. `"view_file"` or a custom tool name).
    pub name: String,
    /// Arguments as a JSON object.
    pub args: serde_json::Value,
    /// Optional call identifier assigned by the backend.
    pub id: Option<String>,
    /// Optional canonical path for file tools.
    #[serde(default)]
    pub canonical_path: Option<String>,
}

/// Error sent over the error channel when the Python stream fails.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamError {
    /// Error message from the Python side.
    pub message: String,
}

impl std::fmt::Display for StreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "stream error: {}", self.message)
    }
}

impl std::error::Error for StreamError {}

/// An ordered event from a response timeline, produced by [`ChatResponseHandle::resolve`].
///
/// Mirrors the Python SDK's `ChatResponse.resolve()` which returns
/// `list[StreamChunk | ToolCall | ToolResult]`.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ResponseEvent {
    /// A text chunk from the model.
    TextChunk(String),
    /// A thinking/reasoning chunk from the model.
    ThoughtChunk(String),
    /// A tool call request from the model.
    ToolCall(ToolCallEvent),
    /// A tool execution result.
    ToolResult(crate::types::ToolResult),
}

/// A chunk from the streaming response, combining text, thought, and tool call events.
///
/// This provides a unified stream of all chunk types, unlike the separate
/// `take_text_stream()` / `take_thought_stream()` / `take_tool_call_stream()`
/// accessors which split events by kind.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StreamChunk {
    /// A text token from the model.
    Text(String),
    /// A thinking/reasoning token.
    Thought(String),
    /// A tool call event.
    ToolCall(ToolCallEvent),
}

/// Handle to a streaming chat response.
///
/// Created by the Python bridge when `agent.chat()` is called. Provides
/// independent channels for text tokens, thinking tokens, and tool call events.
///
/// # Ownership
///
/// The receivers are consumed when you call the corresponding accessor.
/// Each accessor can only be called once — subsequent calls return `None`
/// because the receiver has already been taken.
/// Shared mutable state between the writer and handle.
///
/// Uses `std::sync::Mutex` rather than `tokio::sync::Mutex` because the lock
/// is held only for brief field reads/clones (never across `.await`). This is
/// safe from deadlocks and cheaper than an async mutex.
#[doc(hidden)]
#[derive(Debug, Default)]
pub struct ChatResponseSharedState {
    /// Token usage metadata, populated by the writer after the stream completes.
    pub usage: Option<UsageMetadata>,
    /// Structured output, populated by the writer after the stream completes.
    pub structured_output: Option<serde_json::Value>,
}

/// Grouped receivers for each independent stream channel.
///
/// Extracted from [`ChatResponseHandle`] so the seven channel receivers
/// are logically grouped, keeping the handle's field list manageable.
#[derive(Debug)]
pub(crate) struct StreamReceivers {
    /// Receives text tokens as they arrive from the model.
    text: Option<mpsc::Receiver<String>>,
    /// Receives thinking/reasoning tokens.
    thought: Option<mpsc::Receiver<String>>,
    /// Receives tool call events.
    tool_call: Option<mpsc::Receiver<ToolCallEvent>>,
    /// Receives at most one error if the stream fails.
    error: Option<mpsc::Receiver<StreamError>>,
    /// Receives ordered [`ResponseEvent`]s for [`resolve()`](ChatResponseHandle::resolve).
    event: Option<mpsc::Receiver<ResponseEvent>>,
    /// Receives [`Step`] objects as they are produced.
    step: Option<mpsc::Receiver<Step>>,
    /// Receives unified [`StreamChunk`]s (text, thought, and tool call events).
    chunk: Option<mpsc::Receiver<StreamChunk>>,
}

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
    rx: StreamReceivers,
    /// Token usage metadata, populated after the stream completes.
    usage: Option<UsageMetadata>,
    /// Structured output from a `response_schema`-configured agent.
    structured_output_value: Option<serde_json::Value>,
    /// Shared state to receive metadata updates from the python bridge thread.
    pub(crate) shared_state: Arc<Mutex<ChatResponseSharedState>>,
    /// Semaphore permit that keeps the agent alive while the handle exists.
    pub(crate) keep_alive_permit: Option<tokio::sync::OwnedSemaphorePermit>,
}

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

/// The sending side of a [`ChatResponseHandle`], held by the Python bridge
/// thread that drives the SDK's async iterator.
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
    /// [`ChatResponseHandle::take_step_stream()`]. It will be actively
    /// written once step-level streaming is wired through the command loop.
    pub(crate) step_tx: mpsc::Sender<Step>,
    /// Sends unified [`StreamChunk`]s.
    pub(crate) chunk_tx: mpsc::Sender<StreamChunk>,
    /// Shared state to send metadata updates back to the handle.
    pub(crate) shared_state: Arc<Mutex<ChatResponseSharedState>>,
}

impl ChatResponseWriter {
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
}

/// Default channel buffer size. Large enough to avoid backpressure during
/// normal operation while bounding memory usage.
const CHANNEL_BUFFER: usize = 256;

/// Create a paired `(ChatResponseWriter, ChatResponseHandle)`.
///
/// The writer is handed to the Python bridge thread; the handle is returned
/// to the Rust caller.
#[must_use]
pub fn channel() -> (ChatResponseWriter, ChatResponseHandle) {
    let (text_tx, text_rx) = mpsc::channel(CHANNEL_BUFFER);
    let (thought_tx, thought_rx) = mpsc::channel(CHANNEL_BUFFER);
    let (tool_call_tx, tool_call_rx) = mpsc::channel(CHANNEL_BUFFER);
    let (error_tx, error_rx) = mpsc::channel(1);
    let (event_tx, event_rx) = mpsc::channel(CHANNEL_BUFFER);
    let (step_tx, step_rx) = mpsc::channel(CHANNEL_BUFFER);
    let (chunk_tx, chunk_rx) = mpsc::channel(CHANNEL_BUFFER);

    let shared_state = Arc::new(Mutex::new(ChatResponseSharedState::default()));

    let writer = ChatResponseWriter {
        text_tx,
        thought_tx,
        tool_call_tx,
        error_tx,
        event_tx,
        step_tx,
        chunk_tx,
        shared_state: Arc::clone(&shared_state),
    };

    let handle = ChatResponseHandle {
        keep_alive_permit: None,
        rx: StreamReceivers {
            text: Some(text_rx),
            thought: Some(thought_rx),
            tool_call: Some(tool_call_rx),
            error: Some(error_rx),
            event: Some(event_rx),
            step: Some(step_rx),
            chunk: Some(chunk_rx),
        },
        usage: None,
        structured_output_value: None,
        shared_state,
    };

    (writer, handle)
}

impl ChatResponseHandle {
    /// Take the text token receiver for token-by-token streaming.
    ///
    /// Returns `None` if the receiver was already taken.
    pub const fn take_text_stream(&mut self) -> Option<mpsc::Receiver<String>> {
        self.rx.text.take()
    }

    /// Take the thinking token receiver.
    ///
    /// Returns `None` if the receiver was already taken.
    pub const fn take_thought_stream(&mut self) -> Option<mpsc::Receiver<String>> {
        self.rx.thought.take()
    }

    /// Take the tool call event receiver.
    ///
    /// Returns `None` if the receiver was already taken.
    pub const fn take_tool_call_stream(&mut self) -> Option<mpsc::Receiver<ToolCallEvent>> {
        self.rx.tool_call.take()
    }

    /// Take the raw step receiver.
    ///
    /// Returns `None` if the receiver was already taken.
    /// Prefer [`receive_steps()`](Self::receive_steps) for `StreamExt`-compatible usage.
    pub const fn take_step_stream(&mut self) -> Option<mpsc::Receiver<Step>> {
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
        self.rx.chunk.take().map(ReceiverStream::new)
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

impl ChatResponseWriter {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn streaming_receives_all_tokens_in_order() {
        let (writer, mut handle) = channel();

        let tokens = ["Hello", " ", "world", "!"];
        let expected: String = tokens.iter().copied().collect();

        // Simulate the Python bridge sending tokens
        let send_task = tokio::spawn(async move {
            for token in &["Hello", " ", "world", "!"] {
                writer
                    .text_tx
                    .send((*token).to_owned())
                    .await
                    .expect("send should succeed");
            }
            // Dropping writer closes the channel
        });

        // Consume via the stream receiver
        let mut rx = handle.take_text_stream().expect("should get receiver");
        let mut received = Vec::new();
        while let Some(token) = rx.recv().await {
            received.push(token);
        }

        send_task.await.expect("send task should complete");
        let full: String = received.iter().map(String::as_str).collect();
        assert_eq!(full, expected);
    }

    #[tokio::test]
    async fn text_returns_complete_response() {
        let (writer, handle) = channel();

        tokio::spawn(async move {
            for token in &["The ", "answer ", "is ", "42."] {
                writer
                    .text_tx
                    .send((*token).to_owned())
                    .await
                    .expect("send");
            }
        });

        let text = handle.text().await.expect("should succeed");
        assert_eq!(text, "The answer is 42.");
    }

    #[tokio::test]
    async fn text_returns_empty_when_no_tokens() {
        let (writer, handle) = channel();
        // Drop the writer immediately to close the channel
        drop(writer);

        let text = handle.text().await.expect("should succeed");
        assert!(text.is_empty());
    }

    #[tokio::test]
    async fn stream_error_propagated() {
        let (writer, handle) = channel();

        tokio::spawn(async move {
            writer
                .text_tx
                .send("partial".to_owned())
                .await
                .expect("send");
            writer
                .error_tx
                .send(StreamError {
                    message: "Python exception: quota exceeded".to_owned(),
                })
                .await
                .expect("send error");
        });

        let result = handle.text().await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.message.contains("quota exceeded"));
    }

    #[tokio::test]
    async fn thought_stream_works() {
        let (writer, mut handle) = channel();

        tokio::spawn(async move {
            writer
                .thought_tx
                .send("thinking...".to_owned())
                .await
                .expect("send");
            writer
                .thought_tx
                .send("done.".to_owned())
                .await
                .expect("send");
        });

        let mut rx = handle.take_thought_stream().expect("should get receiver");
        let mut thoughts = Vec::new();
        while let Some(t) = rx.recv().await {
            thoughts.push(t);
        }
        assert_eq!(thoughts, vec!["thinking...", "done."]);
    }

    #[tokio::test]
    async fn tool_call_stream_works() {
        let (writer, mut handle) = channel();

        let event = ToolCallEvent {
            name: "view_file".to_owned(),
            args: serde_json::json!({"path": "/tmp/test.txt"}),
            id: Some("call_1".to_owned()),
            canonical_path: None,
        };

        let event_clone = event.clone();
        tokio::spawn(async move {
            writer.tool_call_tx.send(event_clone).await.expect("send");
        });

        let mut rx = handle.take_tool_call_stream().expect("should get receiver");
        let received = rx.recv().await.expect("should receive event");
        assert_eq!(received.name, "view_file");
        assert_eq!(received.id, Some("call_1".to_owned()));
    }

    #[tokio::test]
    async fn usage_metadata_available_after_finalize() {
        let (writer, mut handle) = channel();
        assert!(handle.usage_metadata().is_none());

        writer.set_usage(UsageMetadata {
            prompt_token_count: Some(100),
            cached_content_token_count: Some(10),
            candidates_token_count: Some(50),
            thoughts_token_count: Some(20),
            total_token_count: Some(170),
        });
        drop(writer);
        handle.finalize();

        let usage = handle.usage_metadata().expect("should have usage");
        assert_eq!(usage.prompt_token_count, Some(100));
        assert_eq!(usage.total_token_count, Some(170));
    }

    #[test]
    fn take_text_stream_returns_none_second_time() {
        let (_writer, mut handle) = channel();
        assert!(handle.take_text_stream().is_some());
        assert!(handle.take_text_stream().is_none());
    }

    #[test]
    fn tool_call_event_serde_roundtrip() {
        let event = ToolCallEvent {
            name: "run_command".to_owned(),
            args: serde_json::json!({"command": "ls"}),
            id: Some("call_42".to_owned()),
            canonical_path: None,
        };
        let json = serde_json::to_string(&event).expect("serialize");
        let parsed: ToolCallEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.name, event.name);
        assert_eq!(parsed.args, event.args);
        assert_eq!(parsed.id, event.id);
    }

    #[test]
    fn take_thought_stream_returns_none_second_time() {
        let (_writer, mut handle) = channel();
        assert!(handle.take_thought_stream().is_some());
        assert!(handle.take_thought_stream().is_none());
    }

    #[test]
    fn take_tool_call_stream_returns_none_second_time() {
        let (_writer, mut handle) = channel();
        assert!(handle.take_tool_call_stream().is_some());
        assert!(handle.take_tool_call_stream().is_none());
    }

    #[test]
    fn stream_error_display() {
        let err = StreamError {
            message: "quota exceeded".to_owned(),
        };
        assert_eq!(format!("{err}"), "stream error: quota exceeded");
    }

    #[test]
    fn stream_error_is_std_error() {
        let err = StreamError {
            message: "test".to_owned(),
        };
        // Verify it implements std::error::Error
        let _: &dyn std::error::Error = &err;
    }

    #[tokio::test]
    async fn concurrent_text_and_thought_streams() {
        let (writer, mut handle) = channel();

        tokio::spawn(async move {
            writer
                .text_tx
                .send("Hello".to_owned())
                .await
                .expect("send text");
            writer
                .thought_tx
                .send("thinking...".to_owned())
                .await
                .expect("send thought");
        });

        let mut text_rx = handle.take_text_stream().expect("text rx");
        let mut thought_rx = handle.take_thought_stream().expect("thought rx");

        let text = text_rx.recv().await.expect("receive text");
        let thought = thought_rx.recv().await.expect("receive thought");

        assert_eq!(text, "Hello");
        assert_eq!(thought, "thinking...");
    }

    #[tokio::test]
    async fn writer_dropped_without_sending_closes_text() {
        let (writer, handle) = channel();
        drop(writer);

        let text = handle.text().await.expect("should succeed");
        assert!(text.is_empty());
    }

    #[tokio::test]
    async fn writer_dropped_without_sending_closes_thought_stream() {
        let (writer, mut handle) = channel();
        drop(writer);

        let mut thought_rx = handle.take_thought_stream().expect("rx");
        assert!(thought_rx.recv().await.is_none());
    }

    #[test]
    fn tool_call_event_without_id() {
        let event = ToolCallEvent {
            name: "custom".to_owned(),
            args: serde_json::json!(null),
            id: None,
            canonical_path: None,
        };
        let json = serde_json::to_string(&event).expect("serialize");
        let parsed: ToolCallEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.name, "custom");
        assert_eq!(parsed.args, serde_json::json!(null));
    }

    #[tokio::test]
    async fn large_token_stream() {
        let (writer, handle) = channel();
        let token_count = 200;

        tokio::spawn(async move {
            for i in 0..token_count {
                writer.text_tx.send(format!("t{i}")).await.expect("send");
            }
        });

        let text = handle.text().await.expect("should succeed");
        // Verify all 200 tokens were collected
        for i in 0..token_count {
            assert!(
                text.contains(&format!("t{i}")),
                "Missing token t{i} in output"
            );
        }
    }

    #[tokio::test]
    async fn resolve_returns_events_in_order() {
        let (writer, handle) = channel();

        let tool_event = ToolCallEvent {
            name: "view_file".to_owned(),
            args: serde_json::json!({"path": "/tmp/x.rs"}),
            id: Some("call_1".to_owned()),
            canonical_path: None,
        };

        let tool_clone = tool_event.clone();
        tokio::spawn(async move {
            writer
                .event_tx
                .send(ResponseEvent::TextChunk("Hello ".to_owned()))
                .await
                .expect("send");
            writer
                .event_tx
                .send(ResponseEvent::ThoughtChunk("hmm".to_owned()))
                .await
                .expect("send");
            writer
                .event_tx
                .send(ResponseEvent::ToolCall(tool_clone))
                .await
                .expect("send");
            writer
                .event_tx
                .send(ResponseEvent::TextChunk("world".to_owned()))
                .await
                .expect("send");
            writer
                .event_tx
                .send(ResponseEvent::ToolResult(crate::types::ToolResult {
                    name: "view_file".to_owned(),
                    id: Some("call_1".to_owned()),
                    result: serde_json::json!({"output": "file contents"}),
                    error: None,
                }))
                .await
                .expect("send");
            // Drop writer to close the channel
        });

        let events = handle.resolve().await;
        assert_eq!(events.len(), 5, "Expected 5 events, got {}", events.len());

        // Verify ordering and types
        assert!(
            matches!(&events[0], ResponseEvent::TextChunk(s) if s == "Hello "),
            "events[0] should be TextChunk(\"Hello \")"
        );
        assert!(
            matches!(&events[1], ResponseEvent::ThoughtChunk(s) if s == "hmm"),
            "events[1] should be ThoughtChunk(\"hmm\")"
        );
        assert!(
            matches!(&events[2], ResponseEvent::ToolCall(tc) if tc.name == "view_file"),
            "events[2] should be ToolCall(view_file)"
        );
        assert!(
            matches!(&events[3], ResponseEvent::TextChunk(s) if s == "world"),
            "events[3] should be TextChunk(\"world\")"
        );
        assert!(
            matches!(&events[4], ResponseEvent::ToolResult(tr) if tr.name == "view_file"),
            "events[4] should be ToolResult(view_file)"
        );
    }

    #[test]
    fn response_event_serde_roundtrip() {
        let events = vec![
            ResponseEvent::TextChunk("hello".to_owned()),
            ResponseEvent::ThoughtChunk("thinking".to_owned()),
            ResponseEvent::ToolCall(ToolCallEvent {
                name: "run_command".to_owned(),
                args: serde_json::json!({"cmd": "ls"}),
                id: Some("c1".to_owned()),
                canonical_path: None,
            }),
            ResponseEvent::ToolResult(crate::types::ToolResult {
                name: "run_command".to_owned(),
                id: Some("c1".to_owned()),
                result: serde_json::json!({"output": "done"}),
                error: None,
            }),
        ];

        let json = serde_json::to_string(&events).expect("serialize");
        let parsed: Vec<ResponseEvent> = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.len(), events.len());
    }

    // ── receive_chunks / receive_steps tests ─────────────────────────────

    #[tokio::test]
    async fn receive_chunks_returns_chunks_in_order() {
        use tokio_stream::StreamExt;

        let (writer, mut handle) = channel();

        tokio::spawn(async move {
            writer
                .chunk_tx
                .send(StreamChunk::Text("hello".to_owned()))
                .await
                .expect("send");
            writer
                .chunk_tx
                .send(StreamChunk::Thought("hmm".to_owned()))
                .await
                .expect("send");
            writer
                .chunk_tx
                .send(StreamChunk::ToolCall(ToolCallEvent {
                    name: "view_file".to_owned(),
                    args: serde_json::json!({}),
                    id: None,
                    canonical_path: None,
                }))
                .await
                .expect("send");
            writer
                .chunk_tx
                .send(StreamChunk::Text(" world".to_owned()))
                .await
                .expect("send");
        });

        let mut stream = handle.receive_chunks().expect("should get stream");
        let mut items = Vec::new();
        while let Some(chunk) = stream.next().await {
            items.push(chunk);
        }

        assert_eq!(items.len(), 4);
        assert!(matches!(&items[0], StreamChunk::Text(t) if t == "hello"));
        assert!(matches!(&items[1], StreamChunk::Thought(t) if t == "hmm"));
        assert!(matches!(&items[2], StreamChunk::ToolCall(tc) if tc.name == "view_file"));
        assert!(matches!(&items[3], StreamChunk::Text(t) if t == " world"));
    }

    #[tokio::test]
    async fn receive_steps_returns_steps() {
        use tokio_stream::StreamExt;

        let (writer, mut handle) = channel();

        tokio::spawn(async move {
            writer
                .step_tx
                .send(crate::types::Step {
                    id: "step-0".to_owned(),
                    step_index: 0,
                    step_type: crate::types::StepType::TextResponse,
                    source: crate::types::StepSource::Model,
                    target: crate::types::StepTarget::User,
                    status: crate::types::StepStatus::Done,
                    content: "Hello".to_owned(),
                    content_delta: "Hello".to_owned(),
                    thinking: String::new(),
                    thinking_delta: String::new(),
                    tool_calls: vec![],
                    error: String::new(),
                    is_complete_response: Some(true),
                    structured_output: None,
                    usage_metadata: None,
                })
                .await
                .expect("send");
        });

        let mut stream = handle.receive_steps().expect("should get stream");
        let step = stream.next().await.expect("should get a step");
        assert_eq!(step.id, "step-0");
        assert_eq!(step.step_type, crate::types::StepType::TextResponse);
        assert_eq!(step.content, "Hello");
    }

    #[tokio::test]
    async fn existing_channels_work_alongside_chunk_stream() {
        use tokio_stream::StreamExt;

        let (writer, mut handle) = channel();

        tokio::spawn(async move {
            // Send through both the dedicated text channel and the chunk channel.
            writer
                .text_tx
                .send("text-tok".to_owned())
                .await
                .expect("send text");
            writer
                .chunk_tx
                .send(StreamChunk::Text("text-tok".to_owned()))
                .await
                .expect("send chunk");
        });

        let mut text_rx = handle.take_text_stream().expect("text rx");
        let text = text_rx.recv().await.expect("receive text");
        assert_eq!(text, "text-tok");

        let mut chunk_stream = handle.receive_chunks().expect("chunk stream");
        let chunk = chunk_stream.next().await.expect("receive chunk");
        assert!(matches!(chunk, StreamChunk::Text(t) if t == "text-tok"));
    }

    #[test]
    fn receive_chunks_returns_none_on_second_call() {
        let (_writer, mut handle) = channel();
        assert!(handle.receive_chunks().is_some());
        assert!(handle.receive_chunks().is_none());
    }

    #[test]
    fn receive_steps_returns_none_on_second_call() {
        let (_writer, mut handle) = channel();
        assert!(handle.receive_steps().is_some());
        assert!(handle.receive_steps().is_none());
    }

    #[test]
    fn stream_chunk_serde_roundtrip() {
        let chunks = vec![
            StreamChunk::Text("hello".to_owned()),
            StreamChunk::Thought("hmm".to_owned()),
            StreamChunk::ToolCall(ToolCallEvent {
                name: "run".to_owned(),
                args: serde_json::json!({"cmd": "ls"}),
                id: Some("c1".to_owned()),
                canonical_path: None,
            }),
        ];
        for chunk in &chunks {
            let json = serde_json::to_string(chunk).expect("serialize");
            let parsed: StreamChunk = serde_json::from_str(&json).expect("deserialize");
            // Verify discriminant matches.
            match (chunk, &parsed) {
                (StreamChunk::Text(a), StreamChunk::Text(b))
                | (StreamChunk::Thought(a), StreamChunk::Thought(b)) => assert_eq!(a, b),
                (StreamChunk::ToolCall(a), StreamChunk::ToolCall(b)) => {
                    assert_eq!(a.name, b.name);
                    assert_eq!(a.id, b.id);
                }
                _ => panic!("variant mismatch after roundtrip"),
            }
        }
    }

    #[tokio::test]
    async fn usage_metadata_populated_from_writer_after_resolve() {
        let (writer, handle) = channel();

        tokio::spawn(async move {
            writer
                .event_tx
                .send(ResponseEvent::TextChunk("hello".to_owned()))
                .await
                .unwrap();
            writer.set_usage(crate::types::UsageMetadata {
                prompt_token_count: Some(5),
                cached_content_token_count: None,
                candidates_token_count: Some(1),
                thoughts_token_count: None,
                total_token_count: Some(6),
            });
            writer.set_structured_output(serde_json::json!({"key": "value"}));
        });

        // resolve() consumes the handle but finalize() runs internally,
        // so we verify via the shared state directly instead.
        let shared = handle.shared_state();
        let events = handle.resolve().await;
        assert_eq!(events.len(), 1);

        let state = shared.lock().expect("lock shared state");
        assert_eq!(state.usage.as_ref().unwrap().total_token_count, Some(6));
        assert_eq!(
            state.structured_output.as_ref().unwrap(),
            &serde_json::json!({"key": "value"})
        );
    }

    #[test]
    fn chat_result_into_string() {
        let (writer, handle) = channel();
        drop(writer);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(handle.text()).unwrap();
        let s: String = result.into();
        assert!(s.is_empty());
    }
}

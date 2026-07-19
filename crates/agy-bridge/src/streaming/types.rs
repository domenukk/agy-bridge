//! Streaming types: chunks, events, errors, and shared state.

use std::{sync::atomic::AtomicBool, time::Duration};

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::types::{Step, UsageMetadata};

/// The result of draining a chat response via [`super::ChatResponseHandle::text()`].
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
    pub(super) text: String,
    pub(super) usage: Option<UsageMetadata>,
    pub(super) structured_output: Option<serde_json::Value>,
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

/// An ordered event from a response timeline, produced by [`super::ChatResponseHandle::resolve`].
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

/// Tracks which streaming "views" a consumer has subscribed to.
///
/// The bridge fans every step out to several independent channels (text,
/// thought, tool-call, the ordered `event` timeline, the unified `chunk`
/// stream, and the raw `step` stream). A given consumer typically attaches to
/// only a subset — e.g. a CLI wants text only, while an orchestrator wants
/// text + thought + step + tool-call.
///
/// Sending to a channel whose receiver is **never drained** fills its bounded
/// buffer and then blocks the writer *forever*, silently stalling the entire
/// stream. To make every consumption pattern deadlock-free, each handle
/// accessor marks its channel as subscribed, and the writer skips fan-out to
/// any unsubscribed channel. Channels that *are* consumed still receive every
/// item (no data loss); only channels nobody listens to are skipped.
///
/// The `error` channel is intentionally omitted: it has capacity 1 and is sent
/// with `try_send` (never blocks), so it needs no gating.
#[derive(Debug, Default)]
pub(crate) struct StreamSubscriptions {
    /// A consumer is draining the text-token stream.
    pub text: AtomicBool,
    /// A consumer is draining the thinking-token stream.
    pub thought: AtomicBool,
    /// A consumer is draining the tool-call stream.
    pub tool_call: AtomicBool,
    /// A consumer is draining the ordered event timeline.
    pub event: AtomicBool,
    /// A consumer is draining the raw step stream.
    pub step: AtomicBool,
    /// A consumer is draining the unified chunk stream.
    pub chunk: AtomicBool,
}

/// Grouped receivers for each independent stream channel.
///
/// Extracted from [`ChatResponseHandle`] so the seven channel receivers
/// are logically grouped, keeping the handle's field list manageable.
#[derive(Debug)]
pub(crate) struct StreamReceivers {
    /// Receives text tokens as they arrive from the model.
    pub(super) text: Option<mpsc::Receiver<String>>,
    /// Receives thinking/reasoning tokens.
    pub(super) thought: Option<mpsc::Receiver<String>>,
    /// Receives tool call events.
    pub(super) tool_call: Option<mpsc::Receiver<ToolCallEvent>>,
    /// Receives at most one error if the stream fails.
    pub(super) error: Option<mpsc::Receiver<StreamError>>,
    /// Receives ordered [`ResponseEvent`]s for [`resolve()`](super::handle::ChatResponseHandle::resolve).
    pub(super) event: Option<mpsc::Receiver<ResponseEvent>>,
    /// Receives [`Step`] objects as they are produced.
    pub(super) step: Option<mpsc::Receiver<Step>>,
    /// Receives unified [`StreamChunk`]s (text, thought, and tool call events).
    pub(super) chunk: Option<mpsc::Receiver<StreamChunk>>,
}

impl StreamReceivers {
    /// Create a new set of receivers from channel endpoints.
    pub(super) fn new(
        text: mpsc::Receiver<String>,
        thought: mpsc::Receiver<String>,
        tool_call: mpsc::Receiver<ToolCallEvent>,
        error: mpsc::Receiver<StreamError>,
        event: mpsc::Receiver<ResponseEvent>,
        step: mpsc::Receiver<Step>,
        chunk: mpsc::Receiver<StreamChunk>,
    ) -> Self {
        Self {
            text: Some(text),
            thought: Some(thought),
            tool_call: Some(tool_call),
            error: Some(error),
            event: Some(event),
            step: Some(step),
            chunk: Some(chunk),
        }
    }
}

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

use self::types::StreamReceivers;
pub use self::{
    handle::ChatResponseHandle,
    types::{
        ChatResponseSharedState, ChatResult, ResponseEvent, StreamChunk, StreamError, ToolCallEvent,
    },
    writer::{ChatResponseWriter, WriterError},
};

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
        rx: StreamReceivers::new(
            text_rx,
            thought_rx,
            tool_call_rx,
            error_rx,
            event_rx,
            step_rx,
            chunk_rx,
        ),
        usage: None,
        structured_output_value: None,
        shared_state,
    };

    (writer, handle)
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

        writer.set_usage(crate::types::UsageMetadata {
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
    fn take_event_stream_returns_none_second_time() {
        let (_writer, mut handle) = channel();
        assert!(handle.take_event_stream().is_some());
        assert!(handle.take_event_stream().is_none());
    }

    #[test]
    fn take_chunk_stream_returns_none_second_time() {
        let (_writer, mut handle) = channel();
        assert!(handle.take_chunk_stream().is_some());
        assert!(handle.take_chunk_stream().is_none());
    }

    /// Regression: `event_tx` uses *blocking* sends and is bounded by
    /// `CHANNEL_BUFFER`. A consumer that drains it concurrently must be able to
    /// receive far more than one buffer's worth of events without the writer
    /// deadlocking. This guards the backpressure-stall class of bug where an
    /// undrained fan-out channel silently halts the entire stream.
    #[tokio::test]
    async fn draining_event_stream_avoids_backpressure_beyond_buffer() {
        let (writer, mut handle) = channel();
        let total = CHANNEL_BUFFER * 3;

        let producer = tokio::spawn(async move {
            for i in 0..total {
                writer
                    .event_tx
                    .send(ResponseEvent::TextChunk(format!("e{i}")))
                    .await
                    .expect("send should not fail while consumer drains");
            }
        });

        let mut rx = handle.take_event_stream().expect("event rx");
        let mut count = 0usize;
        while (rx.recv().await).is_some() {
            count += 1;
        }

        producer.await.expect("producer task");
        assert_eq!(
            count, total,
            "all {total} events must flow when the channel is drained concurrently"
        );
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

    #[tokio::test]
    async fn chat_result_ergonomics() {
        let (writer, handle) = channel();
        tokio::spawn(async move {
            writer
                .text_tx
                .send("hello world".to_owned())
                .await
                .expect("send");
        });

        let result = handle.text().await.expect("text");

        // Deref<Target = str> — str methods work directly on ChatResult.
        assert_eq!(result.len(), 11);
        assert!(result.contains("world"));
        assert_eq!(result.text(), "hello world");

        // PartialEq<&str> and PartialEq<String>.
        assert_eq!(result, "hello world");
        assert_eq!(result, "hello world".to_owned());

        // Display forwards to the inner text.
        assert_eq!(format!("{result}"), "hello world");

        // No usage / structured output was sent.
        assert!(result.usage().is_none());
        assert!(result.structured_output().is_none());

        // into_string() yields the owned inner String.
        assert_eq!(result.into_string(), "hello world");
    }

    // ── Error routing tests (bug: error steps not routed to error_tx) ──────

    #[tokio::test]
    async fn error_step_routed_to_error_channel() {
        // Simulate what route_error_step does: send to both error_tx and step_tx.
        // handle.text() must return Err, not Ok("").
        let (writer, handle) = channel();

        // Destructure the writer so we can drop text_tx explicitly to unblock
        // handle.text()'s drain loop, then send to error_tx and step_tx.
        let ChatResponseWriter {
            text_tx,
            error_tx,
            step_tx,
            ..
        } = writer;

        let producer = async move {
            // Simulate a backend 503 error step — this is what the SDK sends
            // when GenerateContent fails after exhausting retries.
            error_tx
                .try_send(StreamError {
                    message: "Agent execution terminated due to error. (request failed (code 503): APP_ERROR(2))".to_owned(),
                })
                .expect("error_tx should accept");
            step_tx
                .send(crate::types::Step {
                    status: crate::types::StepStatus::Error,
                    error: "Agent execution terminated due to error.".to_owned(),
                    ..crate::types::Step::default()
                })
                .await
                .expect("step_tx should accept");
            // Close the text channel so handle.text() can finish draining.
            drop(text_tx);
        };

        let consumer = handle.text();

        let ((), result) = tokio::join!(producer, consumer);
        let err = result.expect_err("handle.text() must return Err when error step is sent");
        assert!(
            err.message.contains("503") || err.message.contains("Agent execution terminated"),
            "Error message should contain the backend error: {}",
            err.message
        );
    }

    #[tokio::test]
    async fn error_step_without_error_channel_returns_empty_ok() {
        // Verify the OLD behavior (before fix): if only step_tx gets the error
        // but error_tx does NOT, handle.text() returns Ok("").
        // This documents the bug and ensures the fix is needed.
        let (writer, handle) = channel();

        let ChatResponseWriter {
            text_tx, step_tx, ..
        } = writer;

        let producer = async move {
            // Only send to step_tx (NOT error_tx) — the old broken behavior.
            step_tx
                .send(crate::types::Step {
                    status: crate::types::StepStatus::Error,
                    error: "Agent execution terminated".to_owned(),
                    ..crate::types::Step::default()
                })
                .await
                .expect("step_tx should accept");
            // Close text channel so handle.text() can finish draining.
            drop(text_tx);
        };

        let consumer = handle.text();

        // Without the error_tx send, text() returns Ok("") — the bug!
        let ((), result) = tokio::join!(producer, consumer);
        let text =
            result.expect("Without error_tx, text() should return Ok (demonstrating the old bug)");
        assert!(text.is_empty(), "Without error_tx, text should be empty");
    }

    #[tokio::test]
    async fn error_tx_capacity_one_first_error_wins() {
        // error_tx has capacity 1. Multiple sends should not block.
        let (writer, handle) = channel();

        let ChatResponseWriter {
            text_tx, error_tx, ..
        } = writer;

        let producer = async move {
            // First error — should succeed
            error_tx
                .try_send(StreamError {
                    message: "first error".to_owned(),
                })
                .expect("first try_send should succeed");
            // Second error — should fail (channel full), not block
            let second = error_tx.try_send(StreamError {
                message: "second error".to_owned(),
            });
            second.expect_err("Second try_send should fail (channel full)");
            // Close text channel so handle.text() can finish draining.
            drop(text_tx);
        };

        let consumer = handle.text();

        let ((), result) = tokio::join!(producer, consumer);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().message,
            "first error",
            "First error should win"
        );
    }

    #[tokio::test]
    async fn error_step_with_partial_text_still_returns_error() {
        // Even if some text was streamed before the error, the error wins.
        let (writer, handle) = channel();

        let ChatResponseWriter {
            text_tx, error_tx, ..
        } = writer;

        let producer = async move {
            // Some text tokens arrive first
            text_tx
                .send("partial response...".to_owned())
                .await
                .expect("text send");
            // Then backend error
            error_tx
                .try_send(StreamError {
                    message: "connection reset during streaming".to_owned(),
                })
                .expect("error send");
            // text_tx is dropped here, closing the text channel.
        };

        let consumer = handle.text();

        let ((), result) = tokio::join!(producer, consumer);
        let err = result.expect_err("Error should take priority over partial text");
        assert!(
            err.message.contains("connection reset"),
            "Should contain the error message"
        );
    }

    #[tokio::test]
    async fn step_stream_receives_error_steps() {
        // Even with the fix, step consumers should still see error steps.
        use tokio_stream::StreamExt;

        let (writer, mut handle) = channel();

        let error_step = crate::types::Step {
            id: "err-step".to_owned(),
            step_index: 0,
            status: crate::types::StepStatus::Error,
            error: "model 503".to_owned(),
            ..crate::types::Step::default()
        };

        let ChatResponseWriter {
            error_tx, step_tx, ..
        } = writer;

        let producer = async move {
            error_tx
                .try_send(StreamError {
                    message: "model 503".to_owned(),
                })
                .expect("error send");
            step_tx.send(error_step).await.expect("step send");
        };

        let consumer = async {
            let mut stream = handle.receive_steps().expect("should get stream");
            let step = stream.next().await.expect("should get error step");
            assert_eq!(step.status, crate::types::StepStatus::Error);
            assert_eq!(step.error, "model 503");
        };

        tokio::join!(producer, consumer);
    }
}

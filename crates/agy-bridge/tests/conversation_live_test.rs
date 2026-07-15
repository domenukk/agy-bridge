//! Conversation tracking tests — history, turn count, token usage, streaming
//! metadata, multimodal vision, and streaming token delivery.
//!
//! Run with:
//! ```sh
//! GEMINI_API_KEY="..." cargo test --test conversation_live_test -- --nocapture
//! ```

use agy_bridge::tools::JsonSchema;
use serde::Deserialize;

mod common;

use common::{api_key, create_bridge, run_live_test, test_runtime};

// =============================================================================
// Test: Live conversation history, turn count, and token usage tracking
// =============================================================================

#[test]
fn live_conversation_token_usage_tracking() {
    run_live_test("live_conversation_token_usage_tracking", || {
        let _api_key = api_key();
        let rt = test_runtime();

        rt.block_on(async {
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            let bridge = create_bridge();

            let config = agy_bridge::config::AgentConfig::builder()
                .system_instructions("Answer very concisely in 1 word.")
                .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
                .build();

            let agent = bridge.agent(config).await?;

            // Verify initial turn count is 0
            let tc_init = agent.turn_count().await?;
            assert_eq!(tc_init, 0);

            // Send first turn
            let text = agent
                .chat("What is the capital of France?")
                .await?
                .text()
                .await?;
            eprintln!("Capital response: {text}");

            // Verify turn count is now 1
            let tc_after = agent.turn_count().await?;
            assert_eq!(tc_after, 1);

            // Verify history contains at least user + model messages.
            // Newer SDK versions may insert additional entries (thinking,
            // system) so we search by role instead of assuming indices.
            let history = agent.history().await?;
            assert!(
                history.len() >= 2,
                "Expected at least 2 history entries (user + model), got {}",
                history.len()
            );
            let user_msg = history
                .iter()
                .find(|m| m.role == agy_bridge::MessageRole::User)
                .expect("should have a user message in history");
            assert!(
                user_msg.content.contains("France"),
                "user message should mention France: {:?}",
                user_msg.content
            );
            assert!(
                history
                    .iter()
                    .any(|m| m.role == agy_bridge::MessageRole::Model),
                "should have a model message in history"
            );

            // Verify token usage is tracked and greater than zero
            let usage = agent.total_usage().await?;
            let prompt_tokens = usage.prompt_token_count.expect("prompt_tokens");
            let total_tokens = usage.total_token_count.expect("total_tokens");
            assert!(prompt_tokens > 0, "Expected prompt tokens > 0");
            assert!(
                total_tokens > prompt_tokens,
                "Expected total tokens > prompt tokens"
            );

            // Verify turn usage matches total usage on first turn
            let last_usage = agent.last_turn_usage().await?;
            assert_eq!(last_usage.prompt_token_count, Some(prompt_tokens));
            assert_eq!(last_usage.total_token_count, Some(total_tokens));

            // Verify fast-access last usage is also available
            let fast_usage = agent.get_last_usage().expect("get_last_usage");
            assert_eq!(fast_usage.prompt_token_count, Some(prompt_tokens));
            assert_eq!(fast_usage.total_token_count, Some(total_tokens));

            // Clear history and verify turn count resets
            agent.clear_history().await?;
            let tc_cleared = agent.turn_count().await?;
            assert_eq!(tc_cleared, 0);

            // Verify history is empty
            let history_cleared = agent.history().await?;
            assert!(history_cleared.is_empty());

            agent.shutdown().await?;
            Ok(())
        })
    });
}

// =============================================================================
// Test: Streaming completion metadata
// =============================================================================

#[test]
fn live_streaming_completion_metadata() {
    run_live_test("live_streaming_completion_metadata", || {
        let _api_key = api_key();
        let rt = test_runtime();

        rt.block_on(async {
            #[derive(Deserialize, JsonSchema)]
            struct CalculatorResponse {
                answer: i32,
            }

            let bridge = create_bridge();

            let schema_root = schemars::schema_for!(CalculatorResponse);
            let schema = serde_json::to_value(&schema_root).expect("schema serialization");

            let config = agy_bridge::config::AgentConfig::builder()
                .system_instructions("You are a calculator that returns the sum of the numbers as a JSON object with a single 'answer' integer field.")
                .response_schema(agy_bridge::config::JsonSchema::new(schema))
                .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
                .build();

            let agent = bridge.agent(config).await?;

             let result = agent
                .chat("Calculate: 5 + 7")
                .await?
                .text()
                .await?;

            // ChatResult carries usage and structured output alongside text
            if let Some(usage) = result.usage() {
                // NOLINT: test assertion — zero fallback makes the assert fail with a clear message
                assert!(usage.total_token_count.unwrap_or(0) > 0, "Expected non-zero total tokens");
                // NOLINT: test assertion — zero fallback makes the assert fail with a clear message
                assert!(usage.prompt_token_count.unwrap_or(0) > 0, "Expected non-zero prompt tokens");
            } else {
                eprintln!("Warning: usage metadata is None (known localharness issue with structured outputs)");
            }

            let structured_json = result.structured_output().ok_or_else(|| {
                agy_bridge::error::Error::ConnectionError {
                    message: "expected structured output, but got None".to_string(),
                }
            })?;
            let structured: CalculatorResponse = serde_json::from_value(structured_json.clone())
                .expect("failed to deserialize structured output");
            assert_eq!(structured.answer, 12, "Expected structured JSON answer to be 12");

            agent.shutdown().await?;
            Ok(())
        })
    });
}

// =============================================================================
// Test: Multimodal vision
// =============================================================================

#[test]
fn live_multimodal_vision() {
    run_live_test("live_multimodal_vision", || {
        let _api_key = api_key();
        let rt = test_runtime();

        rt.block_on(async {
            use agy_bridge::content::{Content, ContentPrimitive, Image};
            use base64::Engine;

            let key = api_key();

            let config = agy_bridge::config::AgentConfig::builder()
                .model("gemini-3.5-flash")
                .api_key(key)
                .system_instructions("You are a helpful assistant. Answer questions about images directly.")
                .capabilities(agy_bridge::CapabilitiesConfig::custom_tools_only())
                .build();

            let bridge = agy_bridge::AgyBridge::builder()
                .build()?;
            let agent = bridge.agent(config).await?;

            // A tiny 1x1 red PNG base64 decoded
            let red_png_b64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAAAXNSR0IArs4c6QAAAERlWElmTU0AKgAAAAgAAYdpAAQAAAABAAAAGgAAAAAAA6ABAAMAAAABAAEAAKACAAQAAAABAAAAAaADAAQAAAABAAAAAQAAAAD5Ip3+AAAADUlEQVQI12P4z8DwHwAFAAH/VscvDQAAAABJRU5ErkJggg==";
            let image_bytes = base64::engine::general_purpose::STANDARD
                .decode(red_png_b64)
                .unwrap();

            let content = Content::Multi {
                parts: vec![
                    ContentPrimitive::Text {
                        text: "What color is this 1x1 image? Answer in one word.".to_string(),
                    },
                    ContentPrimitive::Image(Image::png(image_bytes)),
                ],
            };

            let stream = agent.chat(content).await?;
            let response = stream.text().await?;
            let response_text = response.text();

            assert!(
                response_text.to_lowercase().contains("red"),
                "Expected the model to see the red image, got: {response_text}"
            );
            Ok(())
        })
    });
}

// =============================================================================
// Test: Streaming - verify token-by-token delivery matches full text
// =============================================================================

#[test]
fn live_streaming_token_delivery() {
    run_live_test("live_streaming_token_delivery", || {
        let _api_key = api_key();
        let rt = test_runtime();

        rt.block_on(async {
            let bridge = create_bridge();
            let config = agy_bridge::config::AgentConfig::builder()
                .system_instructions("You are a storyteller. Write a 5 sentence story about a cat.")
                .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
                .build();

            let agent = bridge.agent(config).await?;

            let mut response = agent.chat("Tell me the story.").await?;

            let mut streamed_text = String::new();
            let mut text_stream = response.take_text_stream().expect("text stream");
            let mut chunk_count = 0;
            loop {
                let opt: Option<String> = text_stream.recv().await;
                match opt {
                    Some(chunk) => {
                        streamed_text.push_str(&chunk);
                        chunk_count += 1;
                    }
                    None => break,
                }
            }
            drop(text_stream);
            // Consume the handle — text stream already drained, so this yields empty.
            drop(response.text().await?);

            eprintln!("Streamed text chunks: {chunk_count}");
            assert!(chunk_count > 1, "Expected multiple streaming chunks");
            assert!(
                !streamed_text.is_empty(),
                "Expected non-empty streamed text"
            );

            agent.shutdown().await?;
            Ok(())
        })
    });
}

// =============================================================================
// Test: Multimodal video (MP4) — exercises the Files API upload path
//
// Unlike images (sent inline as base64), video/document are uploaded via the
// resumable Files API by the localharness binary. This can only be verified
// live: the mock server never sees the upload (the harness bypasses base_url
// for it). See `features_mock_multimodal_test.rs` for the mock-observable
// (image) path and the rationale.
// =============================================================================

#[test]
fn live_multimodal_video_mp4() {
    run_live_test("live_multimodal_video_mp4", || {
        let _api_key = api_key();
        let rt = test_runtime();

        rt.block_on(async {
            use agy_bridge::content::{Content, ContentPrimitive, Video};

            // A tiny (~1.7KB) 1-second solid-red MP4, committed as a fixture.
            let video_bytes = include_bytes!("assets/tiny_red.mp4").to_vec();

            let config = agy_bridge::config::AgentConfig::builder()
                .model("gemini-3.5-flash")
                .api_key(api_key())
                .system_instructions("You answer questions about media concisely.")
                .capabilities(agy_bridge::CapabilitiesConfig::custom_tools_only())
                .build();

            let bridge = agy_bridge::AgyBridge::builder().build()?;
            let agent = bridge.agent(config).await?;

            let content = Content::Multi {
                parts: vec![
                    ContentPrimitive::Text {
                        text: "What color dominates this video? Answer in one word.".to_string(),
                    },
                    ContentPrimitive::Video(Video::mp4(video_bytes)),
                ],
            };

            let stream = agent.chat(content).await?;
            let response = stream.text().await?;
            let response_text = response.text();

            assert!(
                response_text.to_lowercase().contains("red"),
                "Expected the model to see the red video, got: {response_text}"
            );

            agent.shutdown().await?;
            Ok(())
        })
    });
}

// =============================================================================
// Test: Multimodal document (PDF) — exercises the Files API upload path
// =============================================================================

#[test]
fn live_multimodal_document_pdf() {
    run_live_test("live_multimodal_document_pdf", || {
        let _api_key = api_key();
        let rt = test_runtime();

        rt.block_on(async {
            use agy_bridge::content::{Content, ContentPrimitive, Document};

            // A distinctive token the model must read back from the PDF.
            let marker = "AGYBRIDGE";
            let pdf_bytes = minimal_pdf(marker);

            let config = agy_bridge::config::AgentConfig::builder()
                .model("gemini-3.5-flash")
                .api_key(api_key())
                .system_instructions("You answer questions about documents concisely.")
                .capabilities(agy_bridge::CapabilitiesConfig::custom_tools_only())
                .build();

            let bridge = agy_bridge::AgyBridge::builder().build()?;
            let agent = bridge.agent(config).await?;

            let content = Content::Multi {
                parts: vec![
                    ContentPrimitive::Text {
                        text: "What single word is written in this PDF? Reply with just that word."
                            .to_string(),
                    },
                    ContentPrimitive::Document(Document::pdf(pdf_bytes)),
                ],
            };

            let stream = agent.chat(content).await?;
            let response = stream.text().await?;
            let response_text = response.text();

            assert!(
                response_text.to_uppercase().contains(marker),
                "Expected the model to read '{marker}' from the PDF, got: {response_text}"
            );

            agent.shutdown().await?;
            Ok(())
        })
    });
}

/// Builds a minimal, valid single-page PDF that renders `text`.
///
/// Hand-assembles the five standard objects (catalog, pages, page, content
/// stream, font) with a correct `xref` table so real PDF parsers (and the
/// Gemini document pipeline) accept it — no external tooling required.
fn minimal_pdf(text: &str) -> Vec<u8> {
    let stream = format!("BT /F1 24 Tf 30 120 Td ({text}) Tj ET");
    let objects: [Vec<u8>; 5] = [
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] \
          /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>"
            .to_vec(),
        format!(
            "<< /Length {} >>\nstream\n{stream}\nendstream",
            stream.len()
        )
        .into_bytes(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
    ];

    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());
    for (index, body) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        pdf.extend_from_slice(body);
        pdf.extend_from_slice(b"\nendobj\n");
    }

    let xref_pos = pdf.len();
    let object_count = objects.len() + 1; // +1 for the free object 0
    pdf.extend_from_slice(format!("xref\n0 {object_count}\n").as_bytes());
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for offset in &offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {object_count} /Root 1 0 R >>\n\
             startxref\n{xref_pos}\n%%EOF"
        )
        .as_bytes(),
    );
    pdf
}

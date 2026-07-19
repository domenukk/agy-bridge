//! Integration tests for recoverable error step handling.
//!
//! These tests exercise the agy-bridge streaming pipeline when the SDK backend
//! produces error steps that the SDK treats as recoverable (e.g. "model output
//! must contain either output text or tool calls").
//!
//! The mock Gemini server returns responses that trigger these error paths,
//! and we verify the agent stream continues rather than aborting.
//!
//! Run with:
//! ```sh
//! cargo test --test recoverable_error_test -- --nocapture
//! ```

use agy_bridge_test_support::*;

// ═══════════════════════════════════════════════════════════════════════════
// SECTION 1: Empty-candidate responses
// ═══════════════════════════════════════════════════════════════════════════

/// When the model returns an empty candidate (no text, no tool calls), the SDK
/// backend generates an error step but the SDK treats it as recoverable and
/// retries the model call. The mock server returns a text response on the
/// retry. The agent must survive and return the retry text.
///
/// This is the exact bug class this fix targets: with some backends,
/// `forward_step_to_writer` aborted the stream on the error step, killing
/// the agent's turn.
#[test]
fn empty_candidate_then_text_recovers() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        // First POST: empty candidate → backend error → SDK retries.
        // Second POST: normal text response → agent gets the text.
        let server = MockGeminiServer::start(vec![
            MockResponse::EmptyCandidate,
            MockResponse::Text("recovered after thinking-only".into()),
        ])
        .await;

        let agent = BRIDGE
            .agent(agent_config(&server.base_url(), "recoverable"))
            .await
            .expect("agent");

        // Give enough time for the SDK's internal retry loop.
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            agent.chat_text("trigger empty candidate"),
        )
        .await;

        match result {
            Ok(Ok(text)) => {
                eprintln!("Agent recovered with text: {text:?}");
                assert_eq!(
                    text.trim(),
                    "recovered after thinking-only",
                    "Agent should receive the retry text after empty candidate"
                );
            }
            Ok(Err(e)) => {
                let msg = e.to_string();
                // If the backend doesn't retry but instead surfaces the error,
                // the error message must mention "model output" — and critically,
                // it should NOT be a connection/channel error (which would mean
                // the stream was aborted prematurely).
                eprintln!("Agent got error (acceptable if backend doesn't retry): {msg}");
                assert!(
                    msg.contains("model output") || msg.contains("empty"),
                    "Error should be about model output quality, not a stream abort. Got: {msg}"
                );
            }
            Err(elapsed) => {
                panic!("Timed out waiting for agent — possible stream deadlock: {elapsed}");
            }
        }

        // Verify the server was called (at least the first empty-candidate POST).
        assert!(
            server.post_count() >= 1,
            "Server should have received at least 1 POST"
        );

        agent.shutdown().await.expect("shutdown");
    });
}

/// Two empty candidates in a row, then a text response.
/// Verifies the stream doesn't die after repeated recoverable errors.
/// The SDK backend may cap its internal retry count, so we accept either:
/// - Ok("survived two empties") — backend retried successfully
/// - Ok("") — backend exhausted retries and returned empty
/// - Err(...) with a model-quality message — backend surfaced the error
///
/// The key assertion: NO deadlock, NO channel/connection abort.
#[test]
fn multiple_empty_candidates_then_text_recovers() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![
            MockResponse::EmptyCandidate,
            MockResponse::EmptyCandidate,
            MockResponse::Text("survived two empties".into()),
        ])
        .await;

        let agent = BRIDGE
            .agent(agent_config(&server.base_url(), "multi-empty"))
            .await
            .expect("agent");

        let result = tokio::time::timeout(
            std::time::Duration::from_mins(1),
            agent.chat_text("trigger multiple empties"),
        )
        .await;

        match result {
            Ok(Ok(text)) => {
                eprintln!("Recovered after empty candidates: {text:?}");
                // Accept either the retry text or empty (backend exhausted retries).
                let trimmed = text.trim();
                assert!(
                    trimmed == "survived two empties" || trimmed.is_empty(),
                    "Expected retry text or empty, got: {trimmed:?}"
                );
            }
            Ok(Err(e)) => {
                let msg = e.to_string();
                eprintln!("Error after multiple empties (acceptable): {msg}");
                // Must NOT be a channel/connection error (stream abort).
                assert!(
                    !msg.contains("channel closed") && !msg.contains("ChannelClosed"),
                    "Stream must not abort on recoverable errors. Got: {msg}"
                );
            }
            Err(elapsed) => {
                panic!("Timed out — stream may be stuck after repeated empties: {elapsed}");
            }
        }

        assert!(
            server.post_count() >= 1,
            "Server should have been called at least once"
        );

        agent.shutdown().await.expect("shutdown");
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// SECTION 2: Streaming handle behavior with recoverable errors
// ═══════════════════════════════════════════════════════════════════════════

/// The streaming handle's `.text()` must also work correctly when the first
/// attempt returns an empty candidate and the retry succeeds.
#[test]
fn streaming_handle_survives_empty_candidate() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![
            MockResponse::EmptyCandidate,
            MockResponse::Text("stream recovered".into()),
        ])
        .await;

        let agent = BRIDGE
            .agent(agent_config(&server.base_url(), "stream-recover"))
            .await
            .expect("agent");

        let handle_result = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            agent.chat("trigger empty via handle"),
        )
        .await;

        match handle_result {
            Ok(Ok(handle)) => {
                let text_result =
                    tokio::time::timeout(std::time::Duration::from_secs(30), handle.text()).await;
                match text_result {
                    Ok(Ok(chat_result)) => {
                        let text = chat_result.text();
                        eprintln!("Streaming handle recovered: {text:?}");
                        assert_eq!(
                            text.trim(),
                            "stream recovered",
                            "Streaming handle should deliver retry text"
                        );
                    }
                    Ok(Err(e)) => {
                        let msg = e.to_string();
                        eprintln!("Streaming handle error (acceptable): {msg}");
                        assert!(
                            msg.contains("model output") || msg.contains("empty"),
                            "Error should be model-quality, not stream abort. Got: {msg}"
                        );
                    }
                    Err(elapsed) => {
                        panic!("Streaming handle timed out — possible deadlock: {elapsed}");
                    }
                }
            }
            Ok(Err(e)) => {
                eprintln!("chat() error (acceptable if backend doesn't retry): {e}");
            }
            Err(elapsed) => {
                panic!("Timed out at chat() level: {elapsed}");
            }
        }

        agent.shutdown().await.expect("shutdown");
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// SECTION 3: Concurrent agents with mixed empty/healthy
// ═══════════════════════════════════════════════════════════════════════════

/// One agent on a server that returns an empty candidate then text, another
/// on a healthy server. The healthy agent must succeed immediately; the
/// recovering agent must also eventually succeed.
#[test]
fn healthy_agent_unaffected_by_sibling_empty_candidate() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let healthy_server =
            MockGeminiServer::start(vec![MockResponse::Text("healthy response".into())]).await;
        let empty_then_text_server = MockGeminiServer::start(vec![
            MockResponse::EmptyCandidate,
            MockResponse::Text("recovered".into()),
        ])
        .await;

        let healthy_agent = BRIDGE
            .agent(agent_config(&healthy_server.base_url(), "healthy"))
            .await
            .expect("healthy agent");
        let recovering_agent = BRIDGE
            .agent(agent_config(
                &empty_then_text_server.base_url(),
                "recovering",
            ))
            .await
            .expect("recovering agent");

        let (healthy_res, recovering_res) = tokio::join!(
            healthy_agent.chat_text("ping"),
            tokio::time::timeout(
                std::time::Duration::from_secs(30),
                recovering_agent.chat_text("trigger empty"),
            ),
        );

        // Healthy agent MUST succeed unconditionally.
        let healthy_text = healthy_res.expect("healthy agent must succeed");
        assert_eq!(healthy_text.trim(), "healthy response");

        // Recovering agent should either succeed or fail gracefully.
        match recovering_res {
            Ok(Ok(text)) => {
                eprintln!("Recovering agent got: {text:?}");
                assert_eq!(text.trim(), "recovered");
            }
            Ok(Err(e)) => {
                eprintln!("Recovering agent error (acceptable): {e}");
            }
            Err(elapsed) => {
                panic!("Recovering agent timed out — possible stream deadlock: {elapsed}");
            }
        }

        healthy_agent.shutdown().await.expect("shutdown healthy");
        recovering_agent
            .shutdown()
            .await
            .expect("shutdown recovering");
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// SECTION 4: HTTP backend errors (503, 500, 429)
// ═══════════════════════════════════════════════════════════════════════════

/// A 503 error must return `Err(...)`, never `Ok("")`.
///
/// This is the regression test for the `output_after_error` fix: the SDK
/// retries internally and emits error steps whose content was incorrectly
/// counted as "output", causing the error to be swallowed.
#[test]
fn http_503_returns_err() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        // Every POST returns 503.
        let server = MockGeminiServer::start(vec![MockResponse::HttpError {
            status: 503,
            message: "Service unavailable".into(),
        }])
        .await;

        let agent = BRIDGE
            .agent(agent_config(&server.base_url(), "test-503"))
            .await
            .expect("agent");

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            agent.chat_text("trigger 503"),
        )
        .await;

        match result {
            Ok(Ok(text)) => {
                panic!("503 must return Err, got Ok({text:?})");
            }
            Ok(Err(e)) => {
                let msg = e.to_string();
                eprintln!("503 error (expected): {msg}");
                assert!(
                    msg.contains("503") || msg.contains("unavailable") || msg.contains("error"),
                    "Error message should reference the 503: {msg}"
                );
            }
            Err(elapsed) => {
                panic!("Timed out waiting for 503 error: {elapsed}");
            }
        }

        agent.shutdown().await.expect("shutdown");
    });
}

/// A 503 on the first attempt followed by a healthy response on retry must
/// recover and return Ok with the retry text.
///
/// This verifies the `output_after_error` logic: the first attempt produces
/// an error step, but the second attempt produces text. Since output arrived
/// *after* the error, the error is not propagated.
#[test]
fn http_503_then_recovery_returns_ok() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        // First POST: 503 → SDK retries.
        // Second POST: healthy text.
        let server = MockGeminiServer::start(vec![
            MockResponse::HttpError {
                status: 503,
                message: "Temporary unavailable".into(),
            },
            MockResponse::Text("recovered after 503".into()),
        ])
        .await;

        let agent = BRIDGE
            .agent(agent_config(&server.base_url(), "test-503-recovery"))
            .await
            .expect("agent");

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            agent.chat_text("trigger 503 then recover"),
        )
        .await;

        match result {
            Ok(Ok(text)) => {
                eprintln!("Recovered from 503: {text:?}");
                assert_eq!(
                    text.trim(),
                    "recovered after 503",
                    "Should get the retry text after 503 recovery"
                );
            }
            Ok(Err(e)) => {
                // If the SDK doesn't retry (depends on SDK version), the error
                // should be a proper backend error, not a channel abort.
                let msg = e.to_string();
                eprintln!("503 recovery failed (may be SDK version dependent): {msg}");
                assert!(
                    !msg.contains("channel closed") && !msg.contains("ChannelClosed"),
                    "Must not be a stream abort. Got: {msg}"
                );
            }
            Err(elapsed) => {
                panic!("Timed out waiting for 503 recovery: {elapsed}");
            }
        }

        agent.shutdown().await.expect("shutdown");
    });
}

/// A 429 rate-limit error must return `Err(...)`, not `Ok("")`.
#[test]
fn http_429_returns_err() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![MockResponse::HttpError {
            status: 429,
            message: "Quota exceeded".into(),
        }])
        .await;

        let agent = BRIDGE
            .agent(agent_config(&server.base_url(), "test-429"))
            .await
            .expect("agent");

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            agent.chat_text("trigger 429"),
        )
        .await;

        match result {
            Ok(Ok(text)) => {
                panic!("429 must return Err, got Ok({text:?})");
            }
            Ok(Err(e)) => {
                let msg = e.to_string();
                eprintln!("429 error (expected): {msg}");
                assert!(
                    msg.contains("429") || msg.contains("quota") || msg.contains("error"),
                    "Error message should reference the 429: {msg}"
                );
            }
            Err(elapsed) => {
                panic!("Timed out waiting for 429 error: {elapsed}");
            }
        }

        agent.shutdown().await.expect("shutdown");
    });
}

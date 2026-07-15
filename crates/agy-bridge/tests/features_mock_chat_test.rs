//! Mock-server integration tests: plain-chat round-trips against a **test
//! proxy that returns pre-defined values**.
//!
//! Unlike the tool-calling suites, these tests exercise the simplest path —
//! text in, text out — and assert on the *exact* pre-defined values the mock
//! Gemini server hands back, plus what the bridge forwards upstream (system
//! instructions, prompts, and conversation history). **No API key required.**
//!
//! Run with:
//! ```sh
//! cargo test --test features_mock_chat_test -- --nocapture
//! ```

use agy_bridge_test_support::*;

// ═══════════════════════════════════════════════════════════════════════════
// SECTION 1: Text round-trip fidelity
// ═══════════════════════════════════════════════════════════════════════════

/// The pre-defined text the proxy returns must reach the caller verbatim.
#[test]
fn chat_text_returns_predefined_text_verbatim() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let expected = "The capital of France is Paris.";
        let server = MockGeminiServer::start(vec![MockResponse::Text(expected.into())]).await;

        let agent = BRIDGE
            .agent(agent_config(&server.base_url(), "geo"))
            .await
            .expect("agent");

        let text = agent.chat_text("capital of France?").await.expect("chat");
        assert_eq!(
            text.trim(),
            expected,
            "Caller should receive the proxy's pre-defined text verbatim"
        );
        assert_eq!(
            server.post_count(),
            1,
            "Plain chat should issue exactly 1 POST"
        );

        agent.shutdown().await.expect("shutdown");
    });
}

/// Unicode / emoji in the pre-defined value must survive the round-trip intact.
#[test]
fn predefined_unicode_text_round_trip() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let expected = "café ☕ 日本語 🚀 — ok";
        let server = MockGeminiServer::start(vec![MockResponse::Text(expected.into())]).await;

        let agent = BRIDGE
            .agent(agent_config(&server.base_url(), "unicode"))
            .await
            .expect("agent");

        let text = agent.chat_text("say it").await.expect("chat");
        assert_eq!(
            text.trim(),
            expected,
            "Unicode content must be preserved byte-for-byte"
        );

        agent.shutdown().await.expect("shutdown");
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// SECTION 2: What the bridge forwards upstream
// ═══════════════════════════════════════════════════════════════════════════

/// Both the system instruction and the user prompt must appear in the request
/// the bridge sends to the (proxy) backend.
#[test]
fn system_instruction_and_prompt_forwarded_to_backend() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![MockResponse::Text("ack".into())]).await;

        let agent = BRIDGE
            .agent(agent_config(&server.base_url(), "SYSTEM_MARKER_XYZ"))
            .await
            .expect("agent");

        agent.chat_text("PROMPT_MARKER_ABC").await.expect("chat");

        let posts = server.recorded_posts().await;
        assert!(!posts.is_empty(), "Expected at least one recorded POST");
        let body = &posts[0].body;
        assert!(
            body.contains("SYSTEM_MARKER_XYZ"),
            "System instruction should be forwarded, got body: {body}"
        );
        assert!(
            body.contains("PROMPT_MARKER_ABC"),
            "User prompt should be forwarded, got body: {body}"
        );

        agent.shutdown().await.expect("shutdown");
    });
}

/// A second turn on the same agent must carry the first turn's content as
/// conversation history in the upstream request.
#[test]
fn multi_turn_conversation_forwards_history() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![
            MockResponse::Text("First answer.".into()),
            MockResponse::Text("Second answer.".into()),
        ])
        .await;

        let agent = BRIDGE
            .agent(agent_config(&server.base_url(), "chat_history"))
            .await
            .expect("agent");

        let t1 = agent
            .chat_text("Remember the token APPLE_MARKER_T1")
            .await
            .expect("turn 1");
        assert_eq!(t1.trim(), "First answer.");

        let t2 = agent.chat_text("What did I say?").await.expect("turn 2");
        assert_eq!(t2.trim(), "Second answer.");

        assert_eq!(server.post_count(), 2, "Two turns should issue two POSTs");

        let posts = server.recorded_posts().await;
        assert!(
            posts[1].body.contains("APPLE_MARKER_T1"),
            "Second turn's request must include first turn's content as history, \
             got body: {}",
            posts[1].body
        );

        agent.shutdown().await.expect("shutdown");
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// SECTION 3: Streaming & metadata
// ═══════════════════════════════════════════════════════════════════════════

/// Streaming the text channel must assemble to the pre-defined value.
#[test]
fn streaming_text_chunks_assemble_to_predefined_text() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let expected = "Streamed hello world.";
        let server = MockGeminiServer::start(vec![MockResponse::Text(expected.into())]).await;

        let agent = BRIDGE
            .agent(agent_config(&server.base_url(), "streaming"))
            .await
            .expect("agent");

        let mut handle = agent.chat("stream please").await.expect("chat handle");
        let mut stream = handle.take_text_stream().expect("text stream");

        let mut assembled = String::new();
        while let Some(chunk) = stream.recv().await {
            assembled.push_str(&chunk);
        }

        assert_eq!(
            assembled.trim(),
            expected,
            "Streamed chunks should reassemble into the pre-defined text"
        );

        agent.shutdown().await.expect("shutdown");
    });
}

/// The usage metadata attached to the pre-defined response must surface on the
/// resolved `ChatResult`.
#[test]
fn usage_metadata_surfaced_from_predefined_response() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![MockResponse::Text("counted".into())]).await;

        let agent = BRIDGE
            .agent(agent_config(&server.base_url(), "usage"))
            .await
            .expect("agent");

        let handle = agent.chat("count tokens").await.expect("chat handle");
        let result = handle.text().await.expect("text result");

        let usage = result
            .usage()
            .expect("usage metadata should be populated from the mock response");
        // The mock's `text_response_json` reports totalTokenCount = 25.
        assert_eq!(
            usage.total_token_count,
            Some(25),
            "Total token count should reflect the proxy's pre-defined usageMetadata"
        );

        agent.shutdown().await.expect("shutdown");
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// SECTION 4: Isolation
// ═══════════════════════════════════════════════════════════════════════════

/// Two agents pointed at two proxies must each receive their own pre-defined
/// value with no cross-contamination, even when run concurrently.
#[test]
fn two_agents_receive_independent_predefined_text() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server_a =
            MockGeminiServer::start(vec![MockResponse::Text("ALPHA_RESPONSE".into())]).await;
        let server_b =
            MockGeminiServer::start(vec![MockResponse::Text("BETA_RESPONSE".into())]).await;

        let agent_a = BRIDGE
            .agent(agent_config(&server_a.base_url(), "agent_a"))
            .await
            .expect("agent a");
        let agent_b = BRIDGE
            .agent(agent_config(&server_b.base_url(), "agent_b"))
            .await
            .expect("agent b");

        let (r_a, r_b) = tokio::join!(agent_a.chat_text("hi a"), agent_b.chat_text("hi b"));

        assert_eq!(r_a.expect("chat a").trim(), "ALPHA_RESPONSE");
        assert_eq!(r_b.expect("chat b").trim(), "BETA_RESPONSE");

        agent_a.shutdown().await.expect("shutdown a");
        agent_b.shutdown().await.expect("shutdown b");
    });
}

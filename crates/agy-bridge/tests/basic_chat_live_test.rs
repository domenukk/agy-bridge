//! Basic chat live tests — simple round-trip, PING/PONG, text responses.
//!
//! No custom tools involved; just verifying the create → chat → shutdown cycle.
//!
//! Run with:
//! ```sh
//! GEMINI_API_KEY="..." cargo test --test basic_chat_live_test -- --nocapture
//! ```

mod common;

use common::{api_key, create_bridge, run_live_test, test_runtime};

// =============================================================================
// Test: Basic round-trip (create agent → chat → shutdown)
// =============================================================================

#[test]
fn bridge_creates_agent_and_chats() {
    run_live_test("bridge_creates_agent_and_chats", || {
        let _api_key = api_key();
        let rt = test_runtime();

        rt.block_on(async {
            let bridge = create_bridge();

            let config = agy_bridge::config::AgentConfig::builder()
                .system_instructions("Reply with exactly: PONG")
                .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
                .build();

            let agent = bridge.agent(config).await?;
            eprintln!("Created agent: {}", agent.id());

            let text = agent.chat("PING").await?.text().await?;
            eprintln!("Response: {text}");
            assert!(!text.is_empty(), "Expected non-empty response");

            agent.shutdown().await?;
            Ok(())
        })
    });
}

// =============================================================================
// Test: Real round-trip via chat() (no tools)
// =============================================================================

#[test]
fn bridge_real_roundtrip() {
    run_live_test("bridge_real_roundtrip", || {
        let _api_key = api_key();
        let rt = test_runtime();

        rt.block_on(async {
            let bridge = create_bridge();

            let config = agy_bridge::config::AgentConfig::builder()
                .system_instructions("Reply with exactly: hello")
                .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
                .build();

            let agent = bridge.agent(config).await?;
            eprintln!("Created agent: {}", agent.id());

            let text = agent.chat("Say 'hello'").await?.text().await?;
            eprintln!("Real response: {text}");
            assert!(!text.is_empty(), "Expected real response text, got empty");

            agent.shutdown().await?;
            Ok(())
        })
    });
}

// =============================================================================
// Test: Simple chat (PING/PONG)
// =============================================================================

#[test]
fn live_simple_chat() {
    run_live_test("live_simple_chat", || {
        let _api_key = api_key();
        let rt = test_runtime();

        rt.block_on(async {
            let bridge = create_bridge();

            let config = agy_bridge::config::AgentConfig::builder()
                .system_instructions("Reply with exactly: PONG")
                .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
                .build();

            let agent = bridge.agent(config).await?;

            let text = agent.chat("PING").await?.text().await?;
            assert!(!text.is_empty(), "Expected non-empty response");

            agent.shutdown().await?;
            Ok(())
        })
    });
}

// =============================================================================
// Test: Text response via chat()
// =============================================================================

#[test]
fn live_text_response() {
    run_live_test("live_text_response", || {
        let _api_key = api_key();
        let rt = test_runtime();

        rt.block_on(async {
            let bridge = create_bridge();

            let config = agy_bridge::config::AgentConfig::builder()
                .system_instructions("You are a helpful assistant. Answer questions concisely.")
                .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
                .build();

            let agent = bridge.agent(config).await?;

            let text = agent.chat("What color is the sky?").await?.text().await?;
            eprintln!("Response text: {text}");
            assert!(!text.is_empty(), "Expected non-empty response");

            agent.shutdown().await?;
            Ok(())
        })
    });
}

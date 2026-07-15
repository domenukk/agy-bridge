//! Mock-server integration tests: tool calling and the `#[llm_tool]` macro.
//!
//! Tests the full pipeline: Rust → Python SDK → HTTP → Mock Server for
//! tool round-trips, errors, streaming handles, concurrency, and the
//! `#[llm_tool]` proc macro. **No API key required.**
//!
//! Run with:
//! ```sh
//! cargo test --test features_mock_tools_test -- --nocapture
//! ```

use agy_bridge::tools::ToolRegistry;
use agy_bridge_test_support::*;

// ═══════════════════════════════════════════════════════════════════════════
// SECTION 1: Tool Calling
// ═══════════════════════════════════════════════════════════════════════════

/// Single tool round-trip: mock returns functionCall → Rust tool executes →
/// SDK sends functionResponse → mock returns final text.
#[test]
fn tool_single_round_trip() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "add_numbers".into(),
                args: serde_json::json!({"x": 10.0, "y": 32.0}),
            },
            MockResponse::Text("The sum is 42.".into()),
        ])
        .await;

        let mut registry = ToolRegistry::new();
        registry.register(AddTool);

        let agent = BRIDGE
            .agent(agent_config(&server.base_url(), "calc"))
            .tools(registry)
            .await
            .expect("agent");

        let text = agent.chat_text("add 10 and 32").await.expect("chat");
        assert!(text.contains("42"), "Expected '42', got: {text}");
        assert_eq!(server.post_count(), 2, "Expected 2 POSTs");

        agent.shutdown().await.expect("shutdown");
    });
}

/// Two sequential tool calls in one conversation.
#[test]
fn tool_multi_sequential_calls() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "add_numbers".into(),
                args: serde_json::json!({"x": 5.0, "y": 3.0}),
            },
            MockResponse::FunctionCall {
                name: "lookup".into(),
                args: serde_json::json!({"key": "secret"}),
            },
            MockResponse::Text("Sum=8, secret=GAMMA-42.".into()),
        ])
        .await;

        let mut registry = ToolRegistry::new();
        registry.register(AddTool);
        registry.register(LookupTool);

        let agent = BRIDGE
            .agent(agent_config(&server.base_url(), "multi"))
            .tools(registry)
            .await
            .expect("agent");

        let text = agent.chat_text("compute").await.expect("chat");
        assert!(text.contains("GAMMA-42"), "Expected GAMMA-42, got: {text}");
        assert_eq!(server.post_count(), 3, "Expected 3 POSTs");

        agent.shutdown().await.expect("shutdown");
    });
}

/// Tool that returns an error — SDK should send error in functionResponse.
#[test]
fn tool_error_propagated() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "always_fail".into(),
                args: serde_json::json!({"reason": "intentional test failure"}),
            },
            MockResponse::Text("Tool failed as expected.".into()),
        ])
        .await;

        let mut registry = ToolRegistry::new();
        registry.register(AlwaysFailTool);

        let agent = BRIDGE
            .agent(agent_config(&server.base_url(), "err"))
            .tools(registry)
            .await
            .expect("agent");

        let text = agent.chat_text("try tool").await.expect("chat");
        assert!(text.contains("failed"), "Expected 'failed', got: {text}");
        assert_eq!(server.post_count(), 2, "Expected 2 POSTs");

        agent.shutdown().await.expect("shutdown");
    });
}

/// Verify the tool's output appears in the second POST body (functionResponse).
#[test]
fn tool_output_in_function_response() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "lookup".into(),
                args: serde_json::json!({"key": "secret"}),
            },
            MockResponse::Text("Done.".into()),
        ])
        .await;

        let mut registry = ToolRegistry::new();
        registry.register(LookupTool);

        let agent = BRIDGE
            .agent(agent_config(&server.base_url(), "verify"))
            .tools(registry)
            .await
            .expect("agent");

        agent.chat_text("lookup").await.expect("chat");

        let posts = server.recorded_posts().await;
        assert!(posts.len() >= 2, "Expected ≥2 posts");
        assert!(
            posts[1].body.contains("GAMMA-42"),
            "functionResponse should contain tool output 'GAMMA-42', got: {}",
            posts[1].body
        );

        agent.shutdown().await.expect("shutdown");
    });
}

/// Streaming chat handle works with tool calling.
#[test]
fn tool_call_via_streaming_handle() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "add_numbers".into(),
                args: serde_json::json!({"x": 1.0, "y": 2.0}),
            },
            MockResponse::Text("Result: 3".into()),
        ])
        .await;

        let mut registry = ToolRegistry::new();
        registry.register(AddTool);

        let agent = BRIDGE
            .agent(agent_config(&server.base_url(), "stream"))
            .tools(registry)
            .await
            .expect("agent");

        let handle = agent.chat("1+2").await.expect("chat handle");
        let text = handle.text().await.expect("text");
        assert!(text.contains('3'), "Expected '3', got: {text}");

        agent.shutdown().await.expect("shutdown");
    });
}

/// Two agents on the same bridge with different tools + different backends.
#[test]
fn concurrent_agents_different_tools() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server_add = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "add_numbers".into(),
                args: serde_json::json!({"x": 100.0, "y": 200.0}),
            },
            MockResponse::Text("Sum: 300".into()),
        ])
        .await;

        let server_lookup = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "lookup".into(),
                args: serde_json::json!({"key": "status"}),
            },
            MockResponse::Text("Status: operational".into()),
        ])
        .await;

        let mut reg_add = ToolRegistry::new();
        reg_add.register(AddTool);
        let mut reg_lookup = ToolRegistry::new();
        reg_lookup.register(LookupTool);

        let a1 = BRIDGE
            .agent(agent_config(&server_add.base_url(), "adder"))
            .tools(reg_add)
            .await
            .expect("adder");
        let a2 = BRIDGE
            .agent(agent_config(&server_lookup.base_url(), "looker"))
            .tools(reg_lookup)
            .await
            .expect("looker");

        let (r1, r2) = tokio::join!(a1.chat_text("add"), a2.chat_text("look up"));

        assert!(
            r1.expect("add chat").contains("300"),
            "Adder should get 300"
        );
        assert!(
            r2.expect("lookup chat").contains("operational"),
            "Looker should get operational"
        );

        a1.shutdown().await.expect("shutdown a1");
        a2.shutdown().await.expect("shutdown a2");
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// SECTION 7: `#[llm_tool]` proc macro
// ═══════════════════════════════════════════════════════════════════════════

/// The `#[llm_tool]` proc macro generates a `RustTool` implementation
/// from a plain function — verify it works through the full mock pipeline.
#[test]
fn llm_tool_proc_macro_round_trip() {
    use agy_bridge::llm_tool;

    /// Multiplies two integers.
    #[llm_tool]
    fn multiply(
        /// First factor.
        a: i64,
        /// Second factor.
        b: i64,
    ) -> Result<String, agy_bridge::tools::ToolError> {
        Ok(format!("{}", a * b))
    }

    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "multiply".into(),
                args: serde_json::json!({"a": 6, "b": 7}),
            },
            MockResponse::Text("The product is 42.".into()),
        ])
        .await;

        let mut registry = ToolRegistry::new();
        registry.register(Multiply);

        let agent = BRIDGE
            .agent(agent_config(&server.base_url(), "proc_macro"))
            .tools(registry)
            .await
            .expect("agent");

        let text = agent.chat_text("6*7").await.expect("chat");
        assert!(text.contains("42"), "Expected '42', got: {text}");

        agent.shutdown().await.expect("shutdown");
    });
}

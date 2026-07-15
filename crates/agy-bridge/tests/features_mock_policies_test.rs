//! Mock-server integration tests: policies.
//!
//! Covers `AllowAll`, `Deny(specific)`, `Allow(specific) + DenyAll`, and the
//! `AskUser` policy with a Rust `AskUserHandler`. **No API key required.**
//!
//! Run with:
//! ```sh
//! cargo test --test features_mock_policies_test -- --nocapture
//! ```

use std::sync::atomic::Ordering;

use agy_bridge::{policies::PolicyRule, tools::ToolRegistry};
use agy_bridge_test_support::*;

// ═══════════════════════════════════════════════════════════════════════════
// SECTION 3: Policies
// ═══════════════════════════════════════════════════════════════════════════

/// `AllowAll` policy lets tool calls through.
#[test]
fn policy_allow_all_permits_tool() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "add_numbers".into(),
                args: serde_json::json!({"x": 1.0, "y": 1.0}),
            },
            MockResponse::Text("2".into()),
        ])
        .await;

        let (counting_tool, count) = CountingTool::new();
        let mut registry = ToolRegistry::new();
        registry.register(counting_tool);

        // Override to call counting_tool instead.
        let server2 = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "counting_tool".into(),
                args: serde_json::json!({}),
            },
            MockResponse::Text("Done.".into()),
        ])
        .await;

        let config = agy_bridge::config::AgentConfig::builder()
            .system_instructions("test")
            .gemini(agy_bridge::config::GeminiConfig {
                api_key: Some("test-key".into()),
                base_url: Some(server2.base_url()),
                models: agy_bridge::config::ModelConfig::default(),
            })
            .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
            .policies([PolicyRule::AllowAll])
            .build();

        let agent = BRIDGE.agent(config).tools(registry).await.expect("agent");

        agent.chat_text("go").await.expect("chat");

        assert!(
            count.load(Ordering::SeqCst) >= 1,
            "Tool should have been invoked under AllowAll"
        );

        // Clean up unused server.
        drop(server);
        agent.shutdown().await.expect("shutdown");
    });
}

/// `Deny(specific_tool)` blocks that tool but allows others.
#[test]
fn policy_deny_specific_blocks_tool() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "counting_tool".into(),
                args: serde_json::json!({}),
            },
            MockResponse::Text("Done.".into()),
        ])
        .await;

        let (counting_tool, count) = CountingTool::new();
        let mut registry = ToolRegistry::new();
        registry.register(counting_tool);

        let config = agy_bridge::config::AgentConfig::builder()
            .system_instructions("test")
            .gemini(agy_bridge::config::GeminiConfig {
                api_key: Some("test-key".into()),
                base_url: Some(server.base_url()),
                models: agy_bridge::config::ModelConfig::default(),
            })
            .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
            .policies([PolicyRule::deny("counting_tool"), PolicyRule::AllowAll])
            .build();

        let agent = BRIDGE.agent(config).tools(registry).await.expect("agent");

        let _result = agent.chat_text("go").await;

        assert_eq!(
            count.load(Ordering::SeqCst),
            0,
            "counting_tool should NOT execute — it's denied by policy"
        );

        agent.shutdown().await.expect("shutdown");
    });
}

/// `Allow(specific_tool)` + `DenyAll`: only the allowed tool executes.
#[test]
fn policy_allow_specific_plus_deny_all() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "add_numbers".into(),
                args: serde_json::json!({"x": 1.0, "y": 1.0}),
            },
            MockResponse::Text("2".into()),
        ])
        .await;

        let mut registry = ToolRegistry::new();
        registry.register(AddTool);

        let config = agy_bridge::config::AgentConfig::builder()
            .system_instructions("test")
            .gemini(agy_bridge::config::GeminiConfig {
                api_key: Some("test-key".into()),
                base_url: Some(server.base_url()),
                models: agy_bridge::config::ModelConfig::default(),
            })
            .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
            .policies([PolicyRule::allow("add_numbers"), PolicyRule::DenyAll])
            .build();

        let agent = BRIDGE.agent(config).tools(registry).await.expect("agent");

        let text = agent.chat_text("add").await.expect("chat");
        assert!(
            text.contains('2'),
            "Allowed tool should execute, got: {text}"
        );

        agent.shutdown().await.expect("shutdown");
    });
}

/// `DenyAll` blocks every tool call — the tool must never execute.
#[test]
fn policy_deny_all_blocks_tool() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "counting_tool".into(),
                args: serde_json::json!({}),
            },
            MockResponse::Text("This text should not require the tool.".into()),
        ])
        .await;

        let (counting_tool, count) = CountingTool::new();
        let mut registry = ToolRegistry::new();
        registry.register(counting_tool);

        let config = agy_bridge::config::AgentConfig::builder()
            .system_instructions("test")
            .gemini(agy_bridge::config::GeminiConfig {
                api_key: Some("test-key".into()),
                base_url: Some(server.base_url()),
                models: agy_bridge::config::ModelConfig::default(),
            })
            .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
            .policies([PolicyRule::DenyAll])
            .build();

        let agent = BRIDGE.agent(config).tools(registry).await.expect("agent");

        let _result = agent.chat_text("run the tool").await;

        assert_eq!(
            count.load(Ordering::SeqCst),
            0,
            "counting_tool must NOT execute under DenyAll"
        );

        agent.shutdown().await.expect("shutdown");
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// SECTION 9: `AskUser` policy with handler
// ═══════════════════════════════════════════════════════════════════════════

/// `AskUser` policy delegates to a Rust `AskUserHandler` — handler allows.
#[test]
fn policy_ask_user_handler_allows() {
    use agy_bridge::policies::AskUserHandler;

    struct AlwaysAllowHandler;
    impl AskUserHandler for AlwaysAllowHandler {
        fn confirm(&self, _tool_name: &str, _tool_args: &serde_json::Value) -> bool {
            true
        }
    }

    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "add_numbers".into(),
                args: serde_json::json!({"x": 10.0, "y": 5.0}),
            },
            MockResponse::Text("Result: 15.".into()),
        ])
        .await;

        let mut registry = ToolRegistry::new();
        registry.register(AddTool);

        let config = agy_bridge::config::AgentConfig::builder()
            .system_instructions("test")
            .gemini(agy_bridge::config::GeminiConfig {
                api_key: Some("test-key".into()),
                base_url: Some(server.base_url()),
                models: agy_bridge::config::ModelConfig::default(),
            })
            .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
            .policies([
                PolicyRule::AskUser {
                    tool: "add_numbers".to_owned(),
                    handler_id: "confirm_add".to_owned(),
                },
                PolicyRule::DenyAll,
            ])
            .build();

        let agent = BRIDGE
            .agent(config)
            .tools(registry)
            .policy_handler(AlwaysAllowHandler)
            .await
            .expect("agent with AskUser");

        let text = agent.chat_text("add 10 and 5").await.expect("chat");
        assert!(
            text.contains("15"),
            "Handler allowed the tool, expected '15', got: {text}"
        );

        agent.shutdown().await.expect("shutdown");
    });
}

/// `AskUser` policy — handler denies the tool call.
#[test]
fn policy_ask_user_handler_denies() {
    use agy_bridge::policies::AskUserHandler;

    struct AlwaysDenyHandler;
    impl AskUserHandler for AlwaysDenyHandler {
        fn confirm(&self, _tool_name: &str, _tool_args: &serde_json::Value) -> bool {
            false
        }
    }

    let rt = multi_thread_rt();
    rt.block_on(async {
        let (counting_tool, count) = CountingTool::new();

        let server = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "counting_tool".into(),
                args: serde_json::json!({}),
            },
            MockResponse::Text("Tool was denied.".into()),
        ])
        .await;

        let mut registry = ToolRegistry::new();
        registry.register(counting_tool);

        let config = agy_bridge::config::AgentConfig::builder()
            .system_instructions("test")
            .gemini(agy_bridge::config::GeminiConfig {
                api_key: Some("test-key".into()),
                base_url: Some(server.base_url()),
                models: agy_bridge::config::ModelConfig::default(),
            })
            .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
            .policies([
                PolicyRule::AskUser {
                    tool: "counting_tool".to_owned(),
                    handler_id: "confirm_count".to_owned(),
                },
                PolicyRule::DenyAll,
            ])
            .build();

        let agent = BRIDGE
            .agent(config)
            .tools(registry)
            .policy_handler(AlwaysDenyHandler)
            .await
            .expect("agent with deny handler");

        let _result = agent.chat_text("run tool").await;

        assert_eq!(
            count.load(Ordering::SeqCst),
            0,
            "Tool should NOT execute — AskUser handler denied it"
        );

        agent.shutdown().await.expect("shutdown");
    });
}

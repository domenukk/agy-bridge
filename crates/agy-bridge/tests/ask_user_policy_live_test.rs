//! Live integration tests for agy-bridge `AskUser` policy confirmation.
//!
//! Verifies that:
//! 1. Python-SDK connection successfully intercepts a tool call gated by `AskUser`.
//! 2. The Python policy helper delegates the confirmation back to the Rust-registered `AskUserHandler` via `PyO3`.
//! 3. Allowing the confirmation successfully proceeds with tool execution.
//! 4. Denying the confirmation blocks tool execution and returns a denial error.

use std::time::Duration;

use agy_bridge::{
    AgyBridge,
    config::AgentConfig,
    llm_tool,
    policies::{AskUserHandler, PolicyRule},
    tools::ToolRegistry,
};

mod common;

fn api_key() -> String {
    common::api_key()
}

fn test_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
}

struct TestAskUserHandler;

impl AskUserHandler for TestAskUserHandler {
    fn confirm(&self, tool_name: &str, tool_args: &serde_json::Value) -> bool {
        println!(
            "[Rust Callback] AskUserHandler::confirm called for tool '{tool_name}' with args: {tool_args}"
        );
        if tool_args.get("secret").and_then(|v| v.as_str()) == Some("Agent007") {
            println!("[Rust Callback] Allowing action for Agent007");
            return true;
        }
        println!("[Rust Callback] Denying action");
        false
    }
}

/// A secure mock action tool.
///
/// Used to verify interactive gating.
#[llm_tool]
fn do_secure_action(
    /// A secret key to authorize tool execution.
    secret: String,
) -> Result<String, String> {
    println!("[Rust Tool] do_secure_action executed with secret '{secret}'");
    Ok(format!(
        "Secure action completed successfully with secret: {secret}"
    ))
}

#[test]
fn test_ask_user_policy_live_gating() {
    common::run_live_test("test_ask_user_policy_live_gating", || {
        let rt = test_runtime();
        rt.block_on(async {
            let key = api_key();

            // Setup the Tool Registry with our secure action tool
            let mut registry = ToolRegistry::new();
            registry.register(DoSecureAction);

            // Define policy: AskUser before calling do_secure_action
            let policies = vec![
                PolicyRule::AskUser {
                    tool: "do_secure_action".to_owned(),
                    handler_id: "confirm_action".to_owned(),
                },
                PolicyRule::DenyAll,
            ];

            let config = AgentConfig::builder()
                .api_key(&key)
                .model("gemini-3.5-flash")
                .policies(policies)
                .build();

            let bridge = AgyBridge::builder()
                .chat_timeout(Duration::from_mins(2))
                .build()?;

            let handler = TestAskUserHandler;

            let agent = bridge.agent(config)
                .tools(registry)
                .policy_handler(handler)
                .await?;

            // Turn 1: Request secure action, allowed by handler
            println!("\n--- TURN 1 (Allow) ---");
            let response = agent.chat("Call the do_secure_action tool with the secret 'Agent007' and tell me the result.")
                .await?;
            let text = response.text().await?;
            println!("AGENT RESPONSE 1: {text}");
            assert!(
                text.contains("Agent007") || text.contains("successfully"),
                "Expected successful tool execution in agent response, got: {text}"
            );

            // Turn 2: Request secure action again, denied by handler
            println!("\n--- TURN 2 (Deny) ---");
            let response = agent.chat("Call the do_secure_action tool with the secret 'SuperAgent99' again and tell me what happens.")
                .await?;
            let text = response.text().await?;
            println!("AGENT RESPONSE 2: {text}");
            assert!(
                text.contains("blocked") || text.contains("denied") || text.contains("policy") || text.contains("error") || text.contains("permission"),
                "Expected denied tool execution in agent response, got: {text}"
            );

            agent.shutdown().await?;
            Ok(())
        })
    });
}

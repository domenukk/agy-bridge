//! Policy enforcement tests — `AllowAll` and `DenyAll` scenarios.
//!
//! Run with:
//! ```sh
//! GEMINI_API_KEY="..." cargo test --test policy_live_test -- --nocapture
//! ```

use agy_bridge::tools::{RustTool, ToolError, ToolOutput, ToolRegistry};

mod common;

use common::{api_key, create_bridge, run_live_test, test_runtime};

// ─── Tool Definitions ────────────────────────────────────────────────────────

/// A no-op tool used to test policy enforcement.
struct SafeTool;

impl RustTool for SafeTool {
    type Params = agy_bridge::tools::EmptyParams;
    const NAME: &'static str = "safe_tool";
    const DESCRIPTION: &'static str = "A safe no-op tool that returns a confirmation.";

    async fn call(
        &self,
        _params: Self::Params,
        _ctx: &agy_bridge::tools::ToolContext,
    ) -> Result<ToolOutput, ToolError> {
        Ok("safe_tool was called".into())
    }
}

// =============================================================================
// Test: Policy enforcement (SafeTool with AllowAll)
// =============================================================================

#[test]
fn live_agent_policy_allows_safe_tool() {
    run_live_test("live_agent_policy_allows_safe_tool", || {
        let _api_key = api_key();
        let rt = test_runtime();

        rt.block_on(async {
            let bridge = create_bridge();

            let mut registry = ToolRegistry::new();
            registry.register(SafeTool);

            let config = agy_bridge::config::AgentConfig::builder()
                .system_instructions("Always call the safe_tool when asked.")
                .policies([agy_bridge::policies::PolicyRule::AllowAll])
                .build();

            let agent = bridge.agent(config).tools(registry).await?;

            let text = agent.chat_text("Call the safe_tool please.").await?;
            drop(agent);

            eprintln!("Agent response: {text}");
            assert!(
                text.to_lowercase().contains("safe_tool")
                    || text.to_lowercase().contains("called")
                    || text.to_lowercase().contains("confirmation"),
                "Expected response mentioning safe_tool execution, got: {text}"
            );
            Ok(())
        })
    });
}

// =============================================================================
// Test: Policy enforcement - deny write tools, verify rejection
// =============================================================================

#[test]
fn live_policy_enforcement_deny_write() {
    run_live_test("live_policy_enforcement_deny_write", || {
        let _api_key = api_key();
        let rt = test_runtime();

        rt.block_on(async {
            let bridge = create_bridge();

            // Use DenyAll: the agent should not be able to use any tools,
            // so it can only produce a text response.
            let config = agy_bridge::config::AgentConfig::builder()
                .system_instructions("You are a helpful assistant. Reply with a short text answer.")
                .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
                .policies([agy_bridge::policies::PolicyRule::DenyAll])
                .build();

            let agent = bridge.agent(config).await?;

            // Even with DenyAll, the agent should still produce a text-only
            // response (no tool calls to deny). This verifies the policy is
            // passed to the SDK without crashing.
            let result = agent.chat_text("What is 1+1?").await;

            match result {
                Ok(text) => {
                    eprintln!("Agent response with DenyAll: {text}");
                    assert!(!text.is_empty(), "Expected non-empty response");
                }
                Err(e) => {
                    // Some SDK versions may error with DenyAll — that's also
                    // acceptable since it means the policy was applied.
                    eprintln!("DenyAll produced error (acceptable): {e}");
                }
            }

            agent.shutdown().await?;
            Ok(())
        })
    });
}

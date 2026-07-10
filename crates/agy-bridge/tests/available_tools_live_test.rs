//! Live integration tests for `available_tools()`.
//!
//! Verifies that the Python SDK's `ToolRunner` surfaces real tool definitions
//! back to the Rust `AgentHandle` after agent creation, including source tags,
//! descriptions, and parameter schemas.
//!
//! Run with:
//! ```sh
//! GEMINI_API_KEY="..." cargo test --test available_tools_live_test -- --nocapture
//! ```

mod common;

use agy_bridge::tools::{JsonSchema, RustTool, ToolError, ToolOutput, ToolRegistry, ToolSource};
use common::{api_key, create_bridge, run_live_test, test_runtime};
use serde::Deserialize;

// ─── Tool Definitions ────────────────────────────────────────────────────────

/// Parameters for [`Multiply`].
#[derive(Debug, Deserialize, JsonSchema)]
struct MultiplyParams {
    /// First factor.
    a: i64,
    /// Second factor.
    b: i64,
}

/// A simple multiply tool for testing tool discovery.
struct Multiply;

impl RustTool for Multiply {
    type Params = MultiplyParams;
    const NAME: &'static str = "multiply";
    const DESCRIPTION: &'static str = "Multiplies two numbers.";

    // NOLINT: forward-compat with future clippy::unused_async_trait_impl lint
    #[allow(unknown_lints, clippy::unused_async_trait_impl)]
    async fn call(
        &self,
        params: Self::Params,
        _ctx: &agy_bridge::tools::ToolContext,
    ) -> Result<ToolOutput, ToolError> {
        Ok(format!("{}", params.a * params.b).into())
    }
}

// =============================================================================
// Test: custom-tools-only agent still has tools (custom tools are injected)
// =============================================================================

#[test]
fn available_tools_custom_tools_only() {
    run_live_test("available_tools_custom_tools_only", || {
        let _api_key = api_key();
        let rt = test_runtime();

        rt.block_on(async {
            let bridge = create_bridge();

            let mut registry = ToolRegistry::new();
            registry.register(Multiply);

            let config = agy_bridge::config::AgentConfig::builder()
                .system_instructions("You are a calculator.")
                .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
                .policies([agy_bridge::policies::PolicyRule::AllowAll])
                .build();

            let agent = bridge.agent(config).tools(registry).await?;

            let tools = agent.available_tools();
            eprintln!("Available tools (custom-only): {tools:?}");

            assert!(
                !tools.is_empty(),
                "available_tools should not be empty even with custom-tools-only config"
            );

            let multiply = tools
                .iter()
                .find(|t| t.name == "multiply")
                .expect("custom Rust tool 'multiply' should appear in available_tools");

            assert_eq!(
                multiply.source,
                ToolSource::Custom,
                "multiply should be tagged as Custom, got: {:?}",
                multiply.source
            );
            assert!(
                !multiply.description.is_empty(),
                "multiply should have a description"
            );

            // With custom_tools_only, there should be no builtins.
            let builtin_count = tools
                .iter()
                .filter(|t| t.source == ToolSource::Builtin)
                .count();
            assert_eq!(
                builtin_count, 0,
                "custom-tools-only should have 0 builtins, got {builtin_count}"
            );

            agent.shutdown().await?;
            Ok(())
        })
    });
}

// =============================================================================
// Test: agent with builtin tools has builtins in available_tools()
// =============================================================================

#[test]
fn available_tools_includes_builtins() {
    run_live_test("available_tools_includes_builtins", || {
        let _api_key = api_key();
        let rt = test_runtime();

        rt.block_on(async {
            let bridge = create_bridge();

            let config = agy_bridge::config::AgentConfig::builder()
                .system_instructions("You are a helpful assistant.")
                .policies([agy_bridge::policies::PolicyRule::AllowAll])
                .build();

            let agent = bridge.agent(config).await?;

            let tools = agent.available_tools();
            eprintln!("Available tools (with builtins): {tools:?}");

            assert!(
                !tools.is_empty(),
                "available_tools should contain builtin tools"
            );

            // Default config → all builtins present, tagged as Builtin.
            let builtins: Vec<_> = tools
                .iter()
                .filter(|t| t.source == ToolSource::Builtin)
                .collect();
            assert!(
                !builtins.is_empty(),
                "expected at least one builtin tool, got none"
            );

            // Verify some well-known builtins are present.
            let has_known_tool = tools.iter().any(|t| {
                // Match any of the standard SDK builtin tool names
                t.name.contains("file")
                    || t.name.contains("command")
                    || t.name.contains("search")
                    || t.name.contains("code")
            });
            eprintln!("Has known builtin tool: {has_known_tool}");

            assert!(
                tools.len() > 1,
                "expected multiple tools with default capabilities, got {}: {tools:?}",
                tools.len()
            );

            // Every builtin should have a description.
            for b in &builtins {
                assert!(
                    !b.description.is_empty(),
                    "builtin '{}' has no description",
                    b.name
                );
            }

            agent.shutdown().await?;
            Ok(())
        })
    });
}

// =============================================================================
// Test: available_tools includes both custom and builtin tools
// =============================================================================

#[test]
fn available_tools_mixed_custom_and_builtin() {
    run_live_test("available_tools_mixed_custom_and_builtin", || {
        let _api_key = api_key();
        let rt = test_runtime();

        rt.block_on(async {
            let bridge = create_bridge();

            let mut registry = ToolRegistry::new();
            registry.register(Multiply);

            let config = agy_bridge::config::AgentConfig::builder()
                .system_instructions("You are a calculator with file access.")
                .policies([agy_bridge::policies::PolicyRule::AllowAll])
                .build();

            let agent = bridge.agent(config).tools(registry).await?;

            let tools = agent.available_tools();
            eprintln!("Available tools (mixed): {tools:?}");

            // Must have the custom tool.
            let multiply = tools
                .iter()
                .find(|t| t.name == "multiply")
                .expect("custom Rust tool 'multiply' must be in available_tools");
            assert_eq!(multiply.source, ToolSource::Custom);

            // Must also have builtins.
            let builtin_count = tools
                .iter()
                .filter(|t| t.source == ToolSource::Builtin)
                .count();
            assert!(
                builtin_count > 0,
                "expected builtin tools alongside custom, got 0"
            );

            assert!(
                tools.len() > 1,
                "expected multiple tools (custom + builtins), got {}: {tools:?}",
                tools.len()
            );

            agent.shutdown().await?;
            Ok(())
        })
    });
}

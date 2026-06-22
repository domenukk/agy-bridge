//! Demonstrates hook registration and execution using `Hooks`.
//!
//! Hooks let you observe and gate agent lifecycle events. This example
//! shows two equivalent registration patterns, then fires the hooks to
//! show the results.

use agy_bridge::{
    AgyBridge,
    config::AgentConfig,
    hooks::{HookResult, Hooks, PreToolCallDecideContext, PreTurnContext},
};

#[tokio::main]
async fn main() -> Result<(), agy_bridge::error::Error> {
    agy_bridge::load_dotenv();
    // ── 1. Build Hooks with the fluent builder pattern (recommended) ──

    let hooks = Hooks::new()
        // Observer hook: logs every turn's prompt.
        .with_pre_turn("turn_logger", |ctx: &PreTurnContext| {
            println!(
                "  [hook:turn_logger] Turn {} prompt: {}",
                ctx.turn_number, ctx.prompt
            );
        })
        // Gate hook: denies calls to "dangerous_tool".
        .with_pre_tool_call_decide("safety_gate", |ctx: &PreToolCallDecideContext| {
            if ctx.tool_name == "dangerous_tool" {
                println!("  [hook:safety_gate] Blocking tool: {}", ctx.tool_name);
                HookResult::deny("dangerous_tool is not allowed")
            } else {
                HookResult::allow()
            }
        });

    // Alternative: mutable registration pattern (useful when hooks
    // are added conditionally or in loops):
    //
    //   let mut hooks = Hooks::new();
    //   hooks.on_pre_turn("turn_logger", |ctx| { ... });
    //   hooks.on_pre_tool_call_decide("safety_gate", |ctx| { ... });

    // Fire the pre-turn hook manually to demonstrate it.
    hooks.run_pre_turn(&PreTurnContext::new("Hello from the hook demo!", 1));

    // Fire the pre-tool-call-decide hook for a safe tool.
    let safe_result = hooks.run_pre_tool_call_decide(&PreToolCallDecideContext::new(
        "view_file",
        serde_json::Value::Null,
    ));
    println!("  view_file decision: allowed={}", safe_result.allow);

    // Fire the pre-tool-call-decide hook for the blocked tool.
    let blocked_result = hooks.run_pre_tool_call_decide(&PreToolCallDecideContext::new(
        "dangerous_tool",
        serde_json::Value::Null,
    ));
    println!(
        "  dangerous_tool decision: allowed={}, reason='{}'",
        blocked_result.allow, blocked_result.message
    );

    // ── 2. Create the Agent with hooks configured ───────────────────────

    let config = AgentConfig::default();

    let bridge = AgyBridge::builder().build()?;
    let agent = bridge.agent(config).hooks(hooks).await?;

    let prompt = "Describe how hooks can guard agent behaviour.";
    println!("  User: {prompt}");
    let response_text = agent.chat(prompt).await?.text().await?;
    println!("  Agent: {response_text}");

    agent.shutdown().await?;
    Ok(())
}

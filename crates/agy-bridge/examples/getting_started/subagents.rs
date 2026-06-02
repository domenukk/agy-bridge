//! Demonstrates spawning a subagent using `spawn_subagent`.
//!
//! Subagents are child agents that share the parent's runtime. This
//! example creates a parent agent, spawns a child via `spawn_subagent`, sends
//! a chat to the child, and then shuts both down in order.

use agy_bridge::{AgyBridge, config::AgentConfig};

#[tokio::main]
async fn main() -> Result<(), agy_bridge::error::Error> {
    agy_bridge::load_dotenv();
    let bridge = AgyBridge::builder().build()?;

    // ── 1. Create the parent agent ──────────────────────────────────────

    let parent_config = AgentConfig::builder()
        .system_instructions("You are a coordinator agent that delegates tasks to subagents.")
        .build();
    let parent = bridge.agent(parent_config).await?;
    println!("  Parent agent created (id={})", parent.id());

    // ── 2. Spawn a child subagent via spawn_subagent ────────────────────

    let child_config = AgentConfig::builder()
        .system_instructions("You are a math specialist subagent. Answer concisely.")
        .model("gemini-3.5-flash")
        .build();

    let child = parent.spawn_subagent(child_config, None).await?;
    println!("  Child subagent spawned (id={})", child.id());

    // ── 3. Chat with the child subagent ─────────────────────────────────

    let prompt = "What is 17 * 23?";
    println!("  User → child: {prompt}");
    let response_text = child.chat(prompt).await?.text().await?;
    println!("  Child: {response_text}");

    // ── 4. Shut down child first, then parent ───────────────────────────

    child.shutdown().await?;
    println!("  Child shut down.");

    parent.shutdown().await?;
    println!("  Parent shut down.");

    Ok(())
}

//! Demonstrates observability: token usage tracking and conversation history.
//!
//! After a chat turn, query the agent's accumulated token usage and
//! conversation history to understand what happened.

use agy_bridge::AgyBridge;

#[tokio::main]
async fn main() -> Result<(), agy_bridge::error::Error> {
    agy_bridge::load_dotenv();
    let bridge = AgyBridge::builder().build()?;

    let config = agy_bridge::config::AgentConfig::builder().build();
    let agent = bridge.agent(config).await?;

    // Send a prompt and collect the response.
    let prompt = "Explain quantum computing in one sentence.";
    println!("User: {prompt}");
    let text = agent.chat(prompt).await?.text().await?;
    println!("Agent: {text}\n");

    // ── Token usage ─────────────────────────────────────────────────────
    // Accumulated token counts across all turns — useful for cost tracking
    // and quota dashboards.
    let total = agent.total_usage().await?;
    println!("--- Total Token Usage ---");
    println!("{total:#?}\n");

    // Per-turn usage for the most recent turn.
    let last = agent.last_turn_usage().await?;
    println!("--- Last Turn Usage ---");
    println!("{last:#?}\n");

    // ── Conversation history ────────────────────────────────────────────
    // The full message history can be logged for debugging or audit trails.
    let messages = agent.history().await?;
    println!("--- Conversation History ({} messages) ---", messages.len());
    for msg in &messages {
        println!(
            "  [{:?}] {}",
            msg.role,
            msg.content.chars().take(80).collect::<String>()
        );
    }

    // ── Turn count ──────────────────────────────────────────────────────
    let turns = agent.turn_count().await?;
    println!("\nTotal turns completed: {turns}");

    agent.shutdown().await?;
    Ok(())
}

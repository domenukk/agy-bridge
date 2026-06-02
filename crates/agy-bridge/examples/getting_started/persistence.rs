//! Demonstrates conversation persistence — resume a previous session.
//!
//! Saves a conversation ID from the first agent, then creates a second
//! agent that resumes the same conversation using the builder pattern.

use agy_bridge::{AgyBridge, config::AgentConfig};

#[tokio::main]
async fn main() -> Result<(), agy_bridge::error::Error> {
    agy_bridge::load_dotenv();
    let bridge = AgyBridge::builder().build()?;

    // ── Turn 1: tell the agent a fact and save the conversation ──────────
    let config1 = AgentConfig::builder()
        .policies(vec![agy_bridge::policies::PolicyRule::AllowAll])
        .build();
    let agent1 = bridge.agent(config1).await?;

    let _response_text = agent1
        .chat("Remember: my favorite color is blue.")
        .await?
        .text()
        .await?;

    // Capture the conversation ID before shutting down.
    let conversation_id = agent1.conversation_id();
    println!("Saved conversation ID: {conversation_id:?}");
    agent1.shutdown().await?;

    // ── Turn 2: resume the conversation via the builder ─────────────────
    let Some(conv_id) = conversation_id else {
        println!("No conversation ID returned — cannot resume.");
        return Ok(());
    };

    let config2 = AgentConfig::builder().conversation_id(conv_id).build();
    let agent2 = bridge.agent(config2).await?;

    let prompt = "What is my favorite color?";
    println!("User: {prompt}");
    println!("Agent: {}", agent2.chat(prompt).await?.text().await?);

    agent2.shutdown().await?;
    Ok(())
}

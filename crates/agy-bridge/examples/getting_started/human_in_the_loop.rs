//! Demonstrates a minimal human-in-the-loop chat using `agent.chat()`.
//!
//! This example creates an agent and sends a single message, printing
//! the streamed response. Extend with a loop for multi-turn conversation.

use agy_bridge::{AgyBridge, config::AgentConfig};

#[tokio::main]
async fn main() -> Result<(), agy_bridge::error::Error> {
    agy_bridge::load_dotenv();
    let bridge = AgyBridge::builder().build()?;

    let config = AgentConfig::builder()
        .system_instructions("You are a helpful assistant. Be concise.")
        .build();
    let agent = bridge.agent(config).await?;

    println!("  System: Sending a single message to the agent...\n");

    let mut handle = agent.chat("Hello! What can you help me with?").await?;
    let mut stream =
        handle
            .take_text_stream()
            .ok_or_else(|| agy_bridge::error::Error::BackendError {
                message: "Missing stream".into(),
            })?;

    print!("Agent: ");
    while let Some(chunk) = stream.recv().await {
        print!("{chunk}");
    }
    println!();
    drop(handle);

    agent.shutdown().await?;
    Ok(())
}

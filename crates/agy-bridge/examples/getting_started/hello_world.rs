//! Simplest possible agent — create, prompt, print response.
//!
//! This is the minimal "hello world" for `agy-bridge`: initialise the
//! bridge, create an agent with default settings, send a single prompt,
//! and print the response.

use agy_bridge::{AgyBridge, config::AgentConfig};

#[tokio::main]
async fn main() -> Result<(), agy_bridge::error::Error> {
    // Load environment variables from .env
    agy_bridge::load_dotenv();

    // 1. Initialise the bridge (starts the Python runtime).
    let bridge = AgyBridge::builder().build()?;

    // 2. Create an agent with default configuration.
    let agent = bridge.agent(AgentConfig::default()).await?;

    // 3. Send a prompt and collect the full response.
    let prompt = "Say 'Hello World!'";
    println!("User: {prompt}");
    let text = agent.chat(prompt).await?.text().await?;
    println!("Agent: {text}");

    // 4. Shut down gracefully.
    agent.shutdown().await?;
    Ok(())
}

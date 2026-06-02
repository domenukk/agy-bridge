//! Demonstrates adding skill files to an agent's system instructions.
//!
//! Skills are markdown instruction files loaded at agent creation time
//! that extend the agent's capabilities.  This example creates a
//! temporary skill file and configures the agent to use it.

use std::io::Write;

use agy_bridge::{AgyBridge, config::AgentConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    agy_bridge::load_dotenv();
    let bridge = AgyBridge::builder().build()?;

    // Create a temporary skill file that teaches the agent a custom format.
    let mut skill_file = tempfile::NamedTempFile::new()?;
    writeln!(
        skill_file,
        "# Haiku Skill\n\n\
         When asked to write a poem, always respond with a haiku \
         (three lines: 5-7-5 syllables)."
    )?;

    let skill_path = skill_file.path().to_path_buf();
    println!("Skill file: {}", skill_path.display());

    // Configure the agent with the skill loaded.
    let config = AgentConfig::builder().skills(&[skill_path]).build();

    let agent = bridge.agent(config).await?;

    let prompt = "Write a poem about the ocean.";
    println!("User: {prompt}");
    let text = agent.chat(prompt).await?.text().await?;
    println!("Agent: {text}");

    agent.shutdown().await?;
    Ok(())
}

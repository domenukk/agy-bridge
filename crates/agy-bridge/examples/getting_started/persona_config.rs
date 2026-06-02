//! Demonstrates configuring an agent's persona via system instructions.
//!
//! System instructions shape the agent's personality, tone, and
//! expertise.  This example shows both the `Custom` (full override)
//! and `Templated` (identity + sections) approaches.

use agy_bridge::{
    AgyBridge,
    config::{AgentConfig, SystemInstructionSection, SystemInstructions},
};

#[tokio::main]
async fn main() -> Result<(), agy_bridge::error::Error> {
    agy_bridge::load_dotenv();
    let bridge = AgyBridge::builder().build()?;

    // ── Approach 1: Custom — fully replace system instructions ──────────
    let pirate_config = AgentConfig::builder()
        .system_instructions(SystemInstructions::custom(
            "You are a grumpy pirate captain. Respond in pirate dialect. \
             Always mention your parrot, Captain Squawks.",
        ))
        .build();

    let pirate = bridge.agent(pirate_config).await?;
    let prompt = "How do you feel about the sea?";
    println!("--- Custom persona ---");
    println!("User: {prompt}");
    println!("Pirate: {}", pirate.chat(prompt).await?.text().await?);
    pirate.shutdown().await?;

    // ── Approach 2: Templated — identity + appended sections ────────────
    let chef_config = AgentConfig::builder()
        .system_instructions(SystemInstructions::Templated {
            identity: Some("You are Chef Auguste, a French cuisine expert.".into()),
            sections: vec![SystemInstructionSection {
                title: "rules".into(),
                content: "Always suggest wine pairings. Never recommend ketchup.".into(),
            }],
        })
        .build();

    let chef = bridge.agent(chef_config).await?;
    let prompt2 = "What should I cook for dinner?";
    println!("\n--- Templated persona ---");
    println!("User: {prompt2}");
    println!("Chef: {}", chef.chat(prompt2).await?.text().await?);
    chef.shutdown().await?;

    Ok(())
}

//! Demonstrates conversation persistence — resume a previous session.
//!
//! Persistence mirrors the Antigravity SDK exactly: the local harness writes
//! the trajectory into a `save_dir`, and each conversation is identified by an
//! id the harness assigns. To resume, pass that id back as `conversation_id`
//! together with the **same** `save_dir`; the harness reloads that trajectory.
//!
//! The flow is therefore:
//!   1. Start a fresh agent with an explicit `save_dir` (omitting it makes the
//!      SDK use a throwaway temp dir, so nothing survives the process).
//!   2. After the first turn, read [`AgentHandle::conversation_id`] — this is
//!      the SDK-assigned id. Persist it alongside the `save_dir`.
//!   3. Later, build a new agent with that `conversation_id` + the same
//!      `save_dir` to resume where you left off.

use agy_bridge::{AgyBridge, config::AgentConfig};

#[tokio::main]
async fn main() -> Result<(), agy_bridge::error::Error> {
    agy_bridge::load_dotenv();
    let bridge = AgyBridge::builder().build()?;

    // A stable directory shared by both agents is what makes resume possible.
    let save_dir = std::env::temp_dir().join("agy_bridge_persistence_demo");

    // ── Turn 1: tell the agent a fact and persist the conversation ───────
    let config1 = AgentConfig::builder()
        .save_dir(save_dir.clone())
        .policies(vec![agy_bridge::policies::PolicyRule::AllowAll])
        .build();
    let agent1 = bridge.agent(config1).await?;

    let _response_text = agent1
        .chat("Remember: my favorite color is blue.")
        .await?
        .text()
        .await?;

    // The SDK assigns the conversation id during the first turn. Capture it
    // (plus `save_dir`) — together they are all you need to resume later.
    let conversation_id = agent1.conversation_id();
    println!("SDK conversation id: {conversation_id:?}");
    println!("Persisted conversation state to: {}", save_dir.display());
    agent1.shutdown().await?;

    // ── Turn 2: resume by id + the same save_dir ────────────────────────
    let Some(conversation_id) = conversation_id else {
        println!("No conversation id was assigned — cannot resume.");
        return Ok(());
    };

    let config2 = AgentConfig::builder()
        .conversation_id(conversation_id)
        .save_dir(save_dir)
        .policies(vec![agy_bridge::policies::PolicyRule::AllowAll])
        .build();
    let agent2 = bridge.agent(config2).await?;

    let prompt = "What is my favorite color?";
    println!("User: {prompt}");
    println!("Agent: {}", agent2.chat(prompt).await?.text().await?);

    agent2.shutdown().await?;
    Ok(())
}

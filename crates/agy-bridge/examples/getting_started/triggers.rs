//! Demonstrates trigger configuration using `TriggerSet` and `TriggerEntry`.
//!
//! Triggers let the SDK fire periodic or file-change events that wake an agent.
//! This example creates both `Every` and `OnFileChange` triggers, populates a
//! `TriggerSet`, and wires it into the agent configuration.

use agy_bridge::{
    AgyBridge,
    config::AgentConfig,
    triggers::{TriggerConfig, TriggerEntry, TriggerSet},
};

#[tokio::main]
async fn main() -> Result<(), agy_bridge::error::Error> {
    agy_bridge::load_dotenv();
    // ── 1. Build individual trigger entries ──────────────────────────────

    let periodic_trigger = TriggerEntry::new(
        "poll_threads",
        TriggerConfig::every_secs(30),
        "Check threads for new updates",
    );

    let file_trigger = TriggerEntry::new(
        "watch_workspace",
        TriggerConfig::on_file_change("/tmp/agy-bridge-demo"),
        "Files changed in workspace: {changes}",
    );

    println!(
        "  Periodic trigger:  {}",
        periodic_trigger.config.description()
    );
    println!(
        "  File-watch trigger: {}",
        file_trigger.config.description()
    );

    // ── 2. Populate a TriggerSet ────────────────────────────────────────

    let trigger_set = TriggerSet::from([periodic_trigger.clone(), file_trigger.clone()]);

    println!(
        "  TriggerSet has {} triggers: {:?}",
        trigger_set.len(),
        trigger_set.iter().map(|t| &t.name).collect::<Vec<_>>()
    );

    // ── 3. Wire triggers into AgentConfig ───────────────────────────────

    let config = AgentConfig::builder()
        .triggers(Vec::<TriggerEntry>::from(trigger_set))
        .build();

    let bridge = AgyBridge::builder().build()?;
    let agent = bridge.agent(config).await?;

    let prompt = "Describe how triggers automate agent workflows.";
    println!("  User: {prompt}");
    let response_text = agent.chat(prompt).await?.text().await?;
    println!("  Agent: {response_text}");

    agent.shutdown().await?;
    Ok(())
}

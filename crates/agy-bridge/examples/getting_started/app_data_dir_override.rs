//! Demonstrates setting a custom application data directory.
//!
//! By default the SDK stores data under `~/.gemini/antigravity`.
//! The `app_data_dir` builder field lets you redirect it — useful for
//! tests, CI, or multi-tenant deployments.

use agy_bridge::{AgyBridge, config::AgentConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    agy_bridge::load_dotenv();
    let bridge = AgyBridge::builder().build()?;

    // Create a temporary directory to use as the app data dir.
    let tmp_dir = tempfile::tempdir()?;
    let data_dir = tmp_dir.path().to_path_buf();
    println!("Custom app data dir: {}", data_dir.display());

    // Point the agent's data storage to our custom directory.
    let config = AgentConfig::builder()
        .app_data_dir(data_dir.clone())
        .build();

    let agent = bridge.agent(config).await?;

    let prompt = "What is 2 + 2?";
    println!("User: {prompt}");
    let text = agent.chat(prompt).await?.text().await?;
    println!("Agent: {text}");

    // Verify that the SDK wrote data into our custom directory.
    let entries: Vec<_> = std::fs::read_dir(&data_dir)?
        .filter_map(Result::ok)
        .collect();
    println!("Files in custom data dir: {}", entries.len());

    agent.shutdown().await?;
    Ok(())
}

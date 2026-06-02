//! Demonstrates structured (JSON) output using `response_schema` in `AgentConfig`.
//!
//! Instead of relying on system instructions alone, this example uses the
//! native `response_schema` config field to constrain the model's output
//! to a JSON schema derived from a Rust struct.

use agy_bridge::{
    AgyBridge,
    config::{AgentConfig, JsonSchema},
};
use schemars::JsonSchema as JsonSchemaTrait;
use serde::Deserialize;

/// A structured summary with a title and bullet points.
#[derive(Deserialize, JsonSchemaTrait)]
struct Summary {
    title: String,
    bullet_points: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<(), agy_bridge::error::Error> {
    agy_bridge::load_dotenv();
    let bridge = AgyBridge::builder().build()?;

    // ── 1. Generate a JSON Schema from the Rust struct ──────────────────

    let schema_root = schemars::schema_for!(Summary);
    let schema_value = serde_json::to_value(&schema_root).expect("schema serialization");
    println!(
        "  Schema: {}",
        serde_json::to_string_pretty(&schema_value).expect("schema serialization")
    );

    // ── 2. Build AgentConfig with native response_schema ────────────────

    let config = AgentConfig::builder()
        .response_schema(JsonSchema::new(schema_value))
        .build();

    let agent = bridge.agent(config).await?;

    // ── 3. Chat and parse the structured response ───────────────────────

    let prompt = "Summarize the history of the Internet in 3 points.";
    println!("  User: {prompt}");
    let response_text = agent.chat(prompt).await?.text().await?;

    match serde_json::from_str::<Summary>(&response_text) {
        Ok(summary) => {
            println!("  Title: {}", summary.title);
            for (i, point) in summary.bullet_points.iter().enumerate() {
                println!("  {}: {point}", i + 1);
            }
        }
        Err(e) => {
            println!("  Raw output: {response_text}");
            println!("  Parse error: {e}");
        }
    }

    agent.shutdown().await?;
    Ok(())
}

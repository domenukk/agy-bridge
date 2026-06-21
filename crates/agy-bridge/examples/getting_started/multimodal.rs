//! Demonstrates sending multimodal content (text + image) to an agent.
//!
//! Multimodal input combines text and media primitives in a single chat turn.
//! This example constructs a `Content::Multi` value containing both a text
//! prompt and an inline PNG image, then sends it to the agent.

use agy_bridge::{
    AgyBridge,
    config::AgentConfig,
    content::{Content, ContentPrimitive, Image},
};

#[tokio::main]
async fn main() -> Result<(), agy_bridge::error::Error> {
    agy_bridge::load_dotenv();
    let bridge = AgyBridge::builder().build()?;
    let config = AgentConfig::builder()
        .capabilities(agy_bridge::config::CapabilitiesConfig {
            enabled_tools: Some(vec![]),
            enable_subagents: false,
            ..Default::default()
        })
        .build();
    let agent = bridge.agent(config).await?;

    // ── Build multimodal content: text + image ──────────────────────────

    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let image = Image::from_file(manifest_dir.join("blank.png"))?;
    println!(
        "  Image: {} bytes, mime_type={}",
        image.data.len(),
        image.mime_type
    );

    let content = Content::Multi {
        parts: vec![
            ContentPrimitive::Text {
                text: "Describe this image. What colour is the single pixel?".into(),
            },
            ContentPrimitive::Image(image),
        ],
    };

    println!("  User: [Text + 1×1 PNG image]");
    let response_text = agent.chat(content).await?.text().await?;
    println!("  Agent: {response_text}");

    agent.shutdown().await?;
    Ok(())
}

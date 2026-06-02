use agy_bridge::{AgyBridge, config::AgentConfig};

#[tokio::main]
async fn main() -> Result<(), agy_bridge::error::Error> {
    agy_bridge::load_dotenv();
    let bridge = AgyBridge::builder().build()?;
    let config = AgentConfig::builder().build();
    let agent = bridge.agent(config).await?;

    let prompt = "Count to 10 slowly. Say each number explicitly.";
    println!("  User: {prompt}");
    let mut response = agent.chat(prompt).await?;

    print!("  Agent: ");
    if let Some(mut stream) = response.take_text_stream() {
        while let Some(chunk) = stream.recv().await {
            print!("{chunk}");
        }
    }
    println!();

    agent.shutdown().await?;
    Ok(())
}

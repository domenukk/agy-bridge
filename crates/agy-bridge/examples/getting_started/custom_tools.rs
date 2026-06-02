use agy_bridge::{AgyBridge, config::AgentConfig, prelude::*, tools::ToolRegistry};
use serde::Serialize;

/// Weather data returned to the model as JSON (auto-serialized by the macro).
#[derive(Serialize)]
struct WeatherData {
    location: String,
    temp_f: i32,
    condition: String,
}

#[llm_tool]
/// Gets the simulated weather for a location.
fn get_weather(
    /// The location to get weather for.
    location: &str,
) -> Result<WeatherData, String> {
    Ok(WeatherData {
        location: location.to_string(),
        temp_f: 72,
        condition: "Sunny".to_string(),
    })
}

#[tokio::main]
async fn main() -> Result<(), agy_bridge::error::Error> {
    agy_bridge::load_dotenv();
    let bridge = AgyBridge::builder().build()?;

    // Tools defined with #[llm_tool] compile to capitalized structs
    let mut registry = ToolRegistry::new();
    registry.register(GetWeather);
    let config = AgentConfig::builder().build();
    let agent = bridge.agent(config).tools(registry).await?;

    let prompt = "What is the weather in Seattle?";
    println!("  User: {prompt}");
    let response_text = agent.chat(prompt).await?.text().await?;
    println!("  Agent: {response_text}");

    agent.shutdown().await?;
    Ok(())
}

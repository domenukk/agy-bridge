use agy_bridge::prelude::*;

#[llm_tool]
/// Reads the file at the given path and returns its content with lines inverted.
// ⚠️ WARNING: No path validation — do not use in production without sandboxing.
// An LLM agent could request arbitrary filesystem paths (e.g. /etc/passwd).
// In production, validate paths against a workspace allowlist or use
// `PolicyRule::WorkspaceOnly` to restrict file access.
fn read_file_upside_down(
    /// Filesystem path of the file to read.
    path: &str,
) -> Result<String, String> {
    println!("Tool read_file_upside_down called with path: {path}");
    Ok(std::fs::read_to_string(path)
        .map_err(|e| e.to_string())?
        .lines()
        .rev()
        .collect::<Vec<_>>()
        .join("\n"))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    agy_bridge::load_dotenv();
    let bridge = AgyBridge::builder().build()?;

    let mut registry = ToolRegistry::new();
    registry.register(ReadFileUpsideDown);

    let config = AgentConfig::builder()
        .system_instructions(
            "You are a helpful assistant with file reading capabilities.".to_string(),
        )
        .capabilities(CapabilitiesConfig::default())
        .build();

    let agent = bridge.agent(config).tools(registry).await?;

    println!("\nGoogle Antigravity SDK Demo");
    println!("Sending a message to the agent...\n");

    let mut handle = agent
        .chat("Read the file /tmp/test.txt upside down")
        .await?;
    let mut stream = handle.take_text_stream().ok_or("Missing stream")?;

    print!("Agent: ");
    while let Some(chunk) = stream.recv().await {
        print!("{chunk}");
    }
    println!();
    drop(handle);

    agent.shutdown().await?;
    Ok(())
}

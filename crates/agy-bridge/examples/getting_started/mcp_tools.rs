use agy_bridge::{
    AgyBridge,
    config::{AgentConfig, McpServer},
};

#[tokio::main]
async fn main() -> Result<(), agy_bridge::error::Error> {
    agy_bridge::load_dotenv();
    let bridge = AgyBridge::builder().build()?;

    // MCP servers always speak JSON-RPC; the transport (stdio/sse/http) is
    // how the bridge connects to the server process.
    let server = McpServer::stdio("npx")
        .args([
            "-y",
            "@modelcontextprotocol/server-postgres",
            "postgresql://postgres:postgres@localhost:5432/postgres",
        ])
        .build();

    let config = AgentConfig::builder().mcp_servers([server]).build();

    let agent = bridge.agent(config).await?;

    let prompt = "How many tables are in the test database?";
    println!("  User: {prompt}");
    let response_text = agent.chat(prompt).await?.text().await?;
    println!("  Agent: {response_text}");

    agent.shutdown().await?;
    Ok(())
}

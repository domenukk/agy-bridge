//! Interactive CLI with a custom tool and workspace-scoped file access.
//!
//! Demonstrates:
//! - An interactive stdin loop for multi-turn conversation
//! - A custom `#[llm_tool]` that reverses lines of a file
//! - `PolicyRule::WorkspaceOnly` to sandbox file access to `/tmp`
//!
//! Type `exit` or `quit` to end the session.

use std::{
    io::{BufRead, Write},
    path::PathBuf,
};

use agy_bridge::prelude::*;

#[llm_tool]
/// Reads the file at the given path and returns its content with lines inverted.
fn read_file_upside_down(
    /// Filesystem path of the file to read.
    path: &str,
) -> Result<String, String> {
    println!("  [tool] read_file_upside_down({path})");
    Ok(std::fs::read_to_string(path)
        .map_err(|e| e.to_string())?
        .lines()
        .rev()
        .collect::<Vec<_>>()
        .join("\n"))
}

fn read_user_input() -> Option<String> {
    print!("\n  You: ");
    std::io::stdout().flush().ok()?;

    let stdin = std::io::stdin();
    let mut line = String::new();
    stdin.lock().read_line(&mut line).ok()?;

    let trimmed = line.trim().to_owned();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed)
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
        .policies(vec![
            PolicyRule::WorkspaceOnly(vec![PathBuf::from("/tmp")]),
            PolicyRule::AllowAll,
        ])
        .build();

    let agent = bridge.agent(config).tools(registry).await?;

    println!("\n  Interactive CLI (type \"exit\" or \"quit\" to stop)");
    println!("  File access is sandboxed to /tmp via WorkspaceOnly policy.\n");

    loop {
        let Some(input) = read_user_input() else {
            continue;
        };

        if input.eq_ignore_ascii_case("exit") || input.eq_ignore_ascii_case("quit") {
            println!("  Goodbye!");
            break;
        }

        let mut handle = agent.chat(input.as_str()).await?;
        let mut stream =
            handle
                .take_text_stream()
                .ok_or_else(|| agy_bridge::error::Error::BackendError {
                    message: "Missing text stream from agent response".into(),
                })?;

        print!("  Agent: ");
        while let Some(chunk) = stream.recv().await {
            print!("{chunk}");
        }
        println!();
        drop(handle);
    }

    agent.shutdown().await?;
    Ok(())
}

//! Demonstrates a human-in-the-loop multi-turn chat using `agent.chat()`.
//!
//! Starts an interactive loop that reads user input from stdin and streams
//! each agent response token-by-token. Type `exit` or `quit` to end the
//! session.
//!
//! Compare with `streaming.rs`, which sends a single hardcoded prompt.

use std::io::{BufRead, Write};

use agy_bridge::{AgyBridge, config::AgentConfig};

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
async fn main() -> Result<(), agy_bridge::error::Error> {
    agy_bridge::load_dotenv();
    let bridge = AgyBridge::builder().build()?;

    let config = AgentConfig::builder()
        .system_instructions("You are a helpful assistant. Be concise.")
        .build();
    let agent = bridge.agent(config).await?;

    println!("  Interactive chat (type \"exit\" or \"quit\" to stop)\n");

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

use std::{fmt::Write, sync::Arc};

use agy_bridge::{
    prelude::*,
    triggers::{TriggerConfig, TriggerEntry},
};
use tokio::sync::Mutex;

const PASS_TOKEN: &str = "[PASS]";
const MAX_ROUNDS: usize = 4;

#[llm_tool]
/// Decline to respond in the current round.
fn pass_turn() -> Result<String, String> {
    Ok(PASS_TOKEN.to_string())
}

struct ChatRoom {
    history: Arc<Mutex<Vec<(String, String)>>>,
    last_seen: Arc<Mutex<std::collections::HashMap<String, usize>>>,
}

impl ChatRoom {
    fn new(agent_names: &[String]) -> Self {
        let mut ls = std::collections::HashMap::new();
        for n in agent_names {
            ls.insert(n.clone(), 0);
        }
        Self {
            history: Arc::new(Mutex::new(Vec::new())),
            last_seen: Arc::new(Mutex::new(ls)),
        }
    }

    async fn discuss(&self, topic: String, agents: &[(String, Agent)]) {
        println!("\n{}", "=".repeat(60));
        println!("💬 Topic: {topic}");
        println!("{}", "=".repeat(60));

        self.history
            .lock()
            .await
            .push(("User".to_string(), topic.clone()));

        for _ in 0..MAX_ROUNDS {
            let responses = self.sequential_round(agents).await;
            if responses.is_empty() {
                println!("\n  ⏹  All agents passed — discussion complete.");
                break;
            }

            for (name, text) in responses {
                self.history.lock().await.push((name, text));
            }
        }
    }

    async fn sequential_round(&self, agents: &[(String, Agent)]) -> Vec<(String, String)> {
        // Warning: in actual code we'd need parallel borrows on agents, but for a
        // quick script we evaluate sequentially or clone handles. Since AgentHandle isn't Clone
        // (usually unique owner), we cannot easily tokio::spawn with it concurrently unless we use channels
        // But since this is a demonstration of round based chat, we can do sequential chat here
        // to emulate the round, as it's structurally similar logic.

        let mut responses = Vec::new();

        for (name, agent) in agents {
            let mut unseen = Vec::new();
            {
                let hist = self.history.lock().await;
                let mut ls = self.last_seen.lock().await;
                let seen_idx = *ls.get(name).unwrap_or(&0);

                if hist.len() > seen_idx {
                    for (sender, text) in &hist[seen_idx..] {
                        if sender != name {
                            unseen.push((sender.clone(), text.clone()));
                        }
                    }
                }
                ls.insert(name.clone(), hist.len());
            }

            if unseen.is_empty() {
                continue;
            }

            let mut prompt = String::from("New messages from other agents:\n\n");
            for (sender, text) in unseen {
                write!(prompt, "[{sender}]: {text}\n\n").unwrap();
            }
            prompt.push_str("Respond to the latest messages. Address other agents by name. Keep it under 3 sentences. If you have nothing to add, call pass_turn().");

            match agent.chat(prompt).await {
                Ok(response) => match response.text().await {
                    Ok(text) => {
                        let text = text.trim().to_string();
                        if text.contains(PASS_TOKEN) || text.is_empty() {
                            println!("\n  🤐 {name}: (pass)");
                        } else {
                            println!("\n  💬 {name}: {text}");
                            responses.push((name.clone(), text));
                        }
                    }
                    Err(e) => eprintln!("\n  ⚠ {name}: failed to read response: {e}"),
                },
                Err(e) => eprintln!("\n  ⚠ {name}: chat failed: {e}"),
            }
        }

        responses
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    agy_bridge::load_dotenv();
    println!("🏠 Agent Chat Room\n");
    let bridge = AgyBridge::builder().build()?;

    let mut agents = Vec::new();
    let configs = vec![
        (
            "Rational Rita",
            "You are Rational Rita, a research specialist in a group chat with Creative Cal and Skeptical Sam. Give concise, factual answers grounded in evidence.\n\n- Reference others by name.\n- Keep responses under 3 sentences.\n- If topic is purely creative/opinion, call pass_turn().",
        ),
        (
            "Creative Cal",
            "You are Creative Cal, a creative thinker in a group chat with Rational Rita and Skeptical Sam. Offer imaginative perspectives.\n\n- Reference others by name.\n- Keep responses under 3 sentences.\n- If topic is factual, call pass_turn().",
        ),
        (
            "Skeptical Sam",
            "You are Skeptical Sam, a devil's advocate in a group chat with Rational Rita and Creative Cal. Challenge assumptions.\n\n- Reference others by name.\n- Keep responses under 3 sentences.\n- If everyone is balanced enough, call pass_turn().",
        ),
    ];

    let names: Vec<String> = configs.iter().map(|c| c.0.to_string()).collect();

    for (name, instructions) in configs {
        let mut registry = ToolRegistry::new();
        registry.register(PassTurn);
        let config = AgentConfig::builder()
            .system_instructions(instructions.to_string())
            .triggers([TriggerEntry {
                name: "nudge".to_string(),
                config: TriggerConfig::every_secs(60),
                message_template: "The discussion is wrapping up. Make your final point concisely."
                    .to_string(),
            }])
            .build();
        let agent = bridge.agent(config).tools(registry).await?;
        agents.push((name.to_string(), agent));
    }

    let room = ChatRoom::new(&names);

    let topics = vec![
        "Should we colonize Mars, or focus on fixing Earth first?",
        "What's the most overrated programming language?",
    ];

    for topic in topics {
        room.discuss(topic.to_string(), &agents).await;
    }

    println!("\n{}", "=".repeat(60));
    let hist = room.history.lock().await;
    println!("📋 Transcript ({} turns)", hist.len());
    println!("{}", "=".repeat(60));
    for (i, (name, text)) in hist.iter().enumerate() {
        println!("  {}. [{}]: {}", i + 1, name, text);
    }
    drop(hist);

    for (_, agent) in &agents {
        if let Err(e) = agent.shutdown().await {
            eprintln!("⚠ Failed to shut down agent: {e}");
        }
    }

    Ok(())
}

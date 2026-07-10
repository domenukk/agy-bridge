use std::{fmt::Write, sync::Arc};

use agy_bridge::prelude::*;
use tokio::{
    sync::{Mutex, Notify},
    time::{Duration, timeout},
};

const PASS_TOKEN: &str = "[PASS]";
const MAX_CONSECUTIVE_PASSES: usize = 2;
const DISCUSSION_TIMEOUT_SECS: u64 = 60;

#[llm_tool]
/// Decline to respond in the current turn.
///
/// Call this when the topic is outside your expertise, you agree with
/// what's been said, or your input would be redundant.
fn pass_turn() -> Result<String, String> {
    Ok(PASS_TOKEN.to_string())
}

struct AsyncChatRoom {
    history: Arc<Mutex<Vec<(String, String)>>>,
    notify: Arc<Notify>,
    done: Arc<std::sync::atomic::AtomicBool>,
}

impl AsyncChatRoom {
    fn new() -> Self {
        Self {
            history: Arc::new(Mutex::new(Vec::new())),
            notify: Arc::new(Notify::new()),
            done: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    async fn discuss(&self, topic: String, agents: Vec<(String, Agent)>) {
        println!("\n{}", "=".repeat(60));
        println!("💬 Topic: {topic}");
        println!("{}", "=".repeat(60));

        self.history
            .lock()
            .await
            .push(("User".to_string(), topic.clone()));

        let mut tasks = Vec::new();
        for (name, agent) in agents {
            let history_clone = Arc::clone(&self.history);
            let notify_clone = Arc::clone(&self.notify);
            let done_clone = Arc::clone(&self.done);

            tasks.push(tokio::spawn(async move {
                let mut last_seen = 0;
                let mut consecutive_passes = 0;
                let agent = agent;

                while !done_clone.load(std::sync::atomic::Ordering::Acquire) {
                    notify_clone.notified().await;
                    if done_clone.load(std::sync::atomic::Ordering::Acquire) {
                        break;
                    }

                    let mut unseen = Vec::new();
                    {
                        let hist = history_clone.lock().await;
                        if hist.len() > last_seen {
                            for (sender, text) in &hist[last_seen..] {
                                if sender != &name && !text.contains(PASS_TOKEN) && !text.is_empty() {
                                    unseen.push((sender.clone(), text.clone()));
                                }
                            }
                            last_seen = hist.len();
                        }
                    }

                    if unseen.is_empty() {
                        continue;
                    }

                    let mut prompt = String::from("New messages from other agents:\n\n");
                    for (sender, text) in unseen {
                        write!(prompt, "[{sender}]: {text}\n\n").unwrap();
                    }
                    prompt.push_str("Respond to the latest messages. Address other agents by name. Keep it under 3 sentences. If you have nothing to add, call pass_turn().");

                    let Ok(response) = agent.chat(prompt).await else { continue };
                    let text = match response.text().await {
                        Ok(t) => t.trim().to_string(),
                        Err(e) => {
                            eprintln!("  ⚠ {name}: failed to read response text: {e}");
                            continue;
                        }
                    };
                    let is_pass = text.contains(PASS_TOKEN) || text.is_empty();

                    if is_pass {
                        consecutive_passes += 1;
                        println!("\n  🤐 {name}: (pass)");
                    } else {
                        consecutive_passes = 0;
                        println!("\n  💬 {name}: {text}");
                    }

                    {
                        let mut hist = history_clone.lock().await;
                        hist.push((name.clone(), text));
                        last_seen = hist.len();
                        drop(hist);
                        notify_clone.notify_waiters();
                    }

                    if consecutive_passes >= MAX_CONSECUTIVE_PASSES {
                        println!("\n  ✋ {name}: leaving discussion.");
                        break;
                    }
                }

                if let Err(e) = agent.shutdown().await {
                    eprintln!("  ⚠ {name}: shutdown failed: {e}");
                }
            }));
        }

        self.notify.notify_waiters();

        if timeout(
            Duration::from_secs(DISCUSSION_TIMEOUT_SECS),
            futures::future::join_all(tasks),
        )
        .await
        // NOLINT: if-condition checking timeout result — value is not needed, only the timeout status
        .is_err()
        {
            eprintln!("  ⚠ Discussion timed out after {DISCUSSION_TIMEOUT_SECS}s");
        }
        self.done.store(true, std::sync::atomic::Ordering::Release);
        self.notify.notify_waiters();
        println!("\n  ⏹  Discussion finished.");
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    agy_bridge::load_dotenv();
    println!("🏠 Async Agent Chat (no rounds)\n");
    let bridge = AgyBridge::builder().build()?;

    let mut agents = Vec::new();
    let configs = vec![
        (
            "Pragmatic Priya",
            "You are Pragmatic Priya, a senior engineer in a group chat with Visionary Vince and Cautious Cora. Focus on what's technically feasible today.\n\n- Keep responses under 3 sentences.\n- Reference others by name.\n- If topic is purely theoretical, call pass_turn().",
        ),
        (
            "Visionary Vince",
            "You are Visionary Vince, a futurist thinker in a group chat with Pragmatic Priya and Cautious Cora. Paint bold pictures of what's possible in 10-20 years.\n\n- Reference others by name.\n- Keep responses under 3 sentences.\n- If topic is about present-day details, call pass_turn().",
        ),
        (
            "Cautious Cora",
            "You are Cautious Cora, a risk analyst in a group chat with Pragmatic Priya and Visionary Vince. Identify what could go wrong.\n\n- Keep responses under 3 sentences.\n- Flag risks constructively.\n- If everyone is cautious enough, call pass_turn().",
        ),
    ];

    for (name, instructions) in configs {
        let mut registry = ToolRegistry::new();
        registry.register(PassTurn);
        let config = AgentConfig::builder()
            .system_instructions(instructions.to_string())
            .build();
        let agent = bridge.agent(config).tools(registry).await?;
        agents.push((name.to_string(), agent));
    }

    let room = AsyncChatRoom::new();
    room.discuss(
        "Should AI agents be allowed to autonomously deploy code to production?".to_string(),
        agents,
    )
    .await;

    println!("\n{}", "=".repeat(60));
    let hist = room.history.lock().await;
    println!("📋 Transcript ({} turns)", hist.len());
    println!("{}", "=".repeat(60));
    for (i, (name, text)) in hist.iter().enumerate() {
        println!("  {}. [{}]: {}", i + 1, name, text);
    }
    drop(hist);

    Ok(())
}

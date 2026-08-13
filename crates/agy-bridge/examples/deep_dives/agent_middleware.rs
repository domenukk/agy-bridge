//! Agent middleware — rate limiting, audit logging, and error fallback hooks.
//!
//! Demonstrates:
//! - `PreToolCallDecide` hook for per-tool rate limiting
//! - `PostToolCall` hook for audit logging
//! - `OnToolError` hook for error recovery/logging
//! - `Hooks` for callback storage + `HookEntry` config for the builder

use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use agy_bridge::{hooks::HookResult, prelude::*};

#[llm_tool]
/// Look up a user by email address and return their profile.
fn lookup_user(
    /// The email address to look up.
    email: &str,
) -> Result<String, String> {
    Ok(format!(
        "User profile for {email}: name=Alice, role=engineer, team=infra"
    ))
}

#[llm_tool]
/// Send a notification message to a user.
fn send_notification(
    /// The recipient's identifier.
    to: &str,
    /// The notification message body.
    message: &str,
) -> Result<String, String> {
    Ok(format!("Notification sent to {to}: {message}"))
}

/// Check whether `tool_name` has exceeded `max_calls` within `window`.
///
/// Returns `(current_count, is_at_limit)`. If not at the limit, the
/// current timestamp is recorded.
fn check_rate_limit(
    calls: &Mutex<HashMap<String, VecDeque<Instant>>>,
    tool_name: &str,
    max_calls: usize,
    window: Duration,
) -> (usize, bool) {
    let mut map = match calls.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let history = match map.get_mut(tool_name) {
        Some(history) => history,
        None => map.entry(tool_name.to_owned()).or_default(),
    };
    let now = Instant::now();
    // Sliding window: timestamps are monotonically ordered, so pop expired entries from front in O(1).
    while let Some(&front) = history.front() {
        if now.duration_since(front) >= window {
            history.pop_front();
        } else {
            break;
        }
    }
    let at_limit = history.len() >= max_calls;
    if !at_limit {
        history.push_back(now);
    }
    (history.len(), at_limit)
}

fn build_middleware_hooks(audit_log: &Arc<Mutex<Vec<String>>>) -> Hooks {
    let calls = Arc::new(Mutex::new(HashMap::<String, VecDeque<Instant>>::new()));
    let max_calls = 3;
    let window = Duration::from_mins(1);

    let log_audit = Arc::clone(audit_log);
    let log_fallback = Arc::clone(audit_log);

    Hooks::new()
        .with_pre_tool_call_decide("rate_limit", move |ctx| {
            let (count, at_limit) = check_rate_limit(&calls, &ctx.tool_name, max_calls, window);
            if at_limit {
                println!(
                    "  🚫 [RateLimit] Denied {} ({count} calls in {}s)",
                    ctx.tool_name,
                    window.as_secs()
                );
                HookResult::deny(format!(
                    "Rate limit exceeded: {} called {max_calls} times",
                    ctx.tool_name
                ))
            } else {
                HookResult::allow()
            }
        })
        .with_post_tool_call("audit_log", move |ctx| {
            let entry = format!("✅ {}: {}", ctx.tool_name, ctx.result);
            println!("  📝 [Audit] {entry}");
            match log_audit.lock() {
                Ok(mut log) => log.push(entry),
                Err(poisoned) => poisoned.into_inner().push(entry),
            }
        })
        .with_tool_error("fallback", move |ctx| {
            let entry = format!("❌ {}: {}", ctx.tool_name, ctx.error);
            println!("  🔧 [Fallback] {entry}");
            match log_fallback.lock() {
                Ok(mut log) => log.push(entry),
                Err(poisoned) => poisoned.into_inner().push(entry),
            }
            // Log only — let the harness use its default error representation.
            None
        })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    agy_bridge::load_dotenv();
    println!("🔌 Hook Middleware Example\n");
    let bridge = AgyBridge::builder().build()?;

    let mut registry = ToolRegistry::new();
    registry.register(LookupUser);
    registry.register(SendNotification);

    let audit_log = Arc::new(Mutex::new(Vec::new()));
    let hook_runner = build_middleware_hooks(&audit_log);

    let config = AgentConfig::builder()
        .system_instructions("You have access to user lookup and notification tools. Use them as needed. Keep responses under 2 sentences.".to_string())
        .policies(vec![PolicyRule::AllowAll])
        .build();

    let agent = bridge
        .agent(config)
        .tools(registry)
        .hooks(hook_runner)
        .await?;

    println!("\n{}", "=".repeat(60));
    println!("📨 Prompt 1: Normal tool use (audit logged)");
    println!("{}", "=".repeat(60));
    let text1 = agent
        .chat("Send a notification to bob@company.org saying 'Welcome aboard!'.")
        .await?
        .text()
        .await?;
    println!("\n  💬 Agent: {}", text1.trim());

    println!("\n{}", "=".repeat(60));
    println!("📨 Prompt 2: Trigger rate limiting");
    println!("{}", "=".repeat(60));
    let text2 = agent
        .chat("Look up user1@test.com, then user2@test.com, then user3@test.com, then user4@test.com. Use the lookup_user tool for each one.")
        .await?
        .text()
        .await?;
    println!("\n  💬 Agent: {}", text2.trim());

    println!("\n{}", "=".repeat(60));
    {
        let logs = audit_log.lock().unwrap();
        println!("📋 Audit Log ({} entries)", logs.len());
        println!("{}", "=".repeat(60));
        for (i, entry) in logs.iter().enumerate() {
            println!("  {}. {entry}", i + 1);
        }
    }

    agent.shutdown().await?;
    Ok(())
}

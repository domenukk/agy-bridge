//! Agent middleware — rate limiting, audit logging, and error fallback hooks.
//!
//! Demonstrates:
//! - `PreToolCallDecide` hook for per-tool rate limiting
//! - `PostToolCall` hook for audit logging
//! - `OnToolError` hook for error recovery/logging
//! - `Hooks` for callback storage + `HookEntry` config for the builder

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use agy_bridge::hooks::HookEntry;
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
/// Returns `(current_count, is_at_limit)`.  If not at the limit, the
/// current timestamp is recorded.
fn check_rate_limit(
    calls: &Mutex<std::collections::HashMap<String, Vec<Instant>>>,
    tool_name: &str,
    max_calls: usize,
    window: Duration,
) -> (usize, bool) {
    let mut map = calls.lock().unwrap();
    let history = map.entry(tool_name.to_owned()).or_default();
    let now = Instant::now();
    history.retain(|t| now.duration_since(*t) < window);
    let at_limit = history.len() >= max_calls;
    if !at_limit {
        history.push(now);
    }
    let count = history.len();
    drop(map);
    (count, at_limit)
}

fn register_rate_limit_hook(runner: &mut Hooks, entries: &mut Vec<HookEntry>) {
    let calls = Arc::new(Mutex::new(
        std::collections::HashMap::<String, Vec<Instant>>::new(),
    ));
    let max_calls = 3;
    let window = Duration::from_mins(1);

    runner.on_pre_tool_call_decide("rate_limit", move |ctx| {
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
    });
    entries.push(HookEntry {
        name: "rate_limit".to_string(),
        point: HookPoint::PreToolCallDecide,
        callback_id: "rate_limit".to_string(),
    });
}

fn register_audit_log_hook(
    runner: &mut Hooks,
    entries: &mut Vec<HookEntry>,
    audit_log: &Arc<Mutex<Vec<String>>>,
) {
    let log = Arc::clone(audit_log);
    runner.on_post_tool_call("audit_log", move |ctx| {
        let entry = format!("✅ {}: {}", ctx.tool_name, ctx.result);
        println!("  📝 [Audit] {entry}");
        log.lock().unwrap().push(entry);
    });
    entries.push(HookEntry {
        name: "audit_log".to_string(),
        point: HookPoint::PostToolCall,
        callback_id: "audit_log".to_string(),
    });
}

fn register_fallback_hook(
    runner: &mut Hooks,
    entries: &mut Vec<HookEntry>,
    audit_log: &Arc<Mutex<Vec<String>>>,
) {
    let log = Arc::clone(audit_log);
    runner.on_tool_error("fallback", move |ctx| {
        let entry = format!("❌ {}: {}", ctx.tool_name, ctx.error);
        println!("  🔧 [Fallback] {entry}");
        log.lock().unwrap().push(entry);
    });
    entries.push(HookEntry {
        name: "fallback".to_string(),
        point: HookPoint::OnToolError,
        callback_id: "fallback".to_string(),
    });
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
    let mut hook_runner = Hooks::new();
    let mut hook_entries = Vec::new();
    register_rate_limit_hook(&mut hook_runner, &mut hook_entries);
    register_audit_log_hook(&mut hook_runner, &mut hook_entries, &audit_log);
    register_fallback_hook(&mut hook_runner, &mut hook_entries, &audit_log);

    let config = AgentConfig::builder()
        .system_instructions("You have access to user lookup and notification tools. Use them as needed. Keep responses under 2 sentences.".to_string())
        .hooks(hook_entries.clone())
        .policies(vec![PolicyRule::AllowAll])
        .build();

    let agent = bridge.agent(config).tools(registry).await?;

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

//! Demonstrates policy-based tool access control using `PolicyRule` and `PolicySet`.
//!
//! Policies let you selectively allow or deny tool access per-agent. This
//! example builds a `PolicySet` with explicit Allow and Deny rules, evaluates
//! several tools against it, and wires the rules into an `AgentConfig`.

use agy_bridge::{
    AgyBridge,
    config::AgentConfig,
    policies::{PolicyDecision, PolicyRule, PolicySet},
};

#[tokio::main]
async fn main() -> Result<(), agy_bridge::error::Error> {
    agy_bridge::load_dotenv();
    // ── 1. Build a PolicySet with Allow AND Deny rules ──────────────────

    let mut policy_set = PolicySet::new();
    // Allow read-only tools explicitly.
    policy_set.push(PolicyRule::allow("view_file"))?;
    policy_set.push(PolicyRule::allow("list_directory"))?;
    // Deny dangerous tools explicitly.
    policy_set.push(PolicyRule::deny("run_command"))?;
    policy_set.push(PolicyRule::deny("edit_file"))?;
    // Deny everything else by default.
    policy_set.push(PolicyRule::DenyAll)?;

    // ── 2. Evaluate several tools against the policy ────────────────────

    let tools_to_check = [
        "view_file",
        "list_directory",
        "run_command",
        "edit_file",
        "create_file",
    ];
    for tool in &tools_to_check {
        let decision = policy_set.evaluate(tool);
        let status = match decision {
            PolicyDecision::Allow => "ALLOWED",
            PolicyDecision::Deny => "DENIED",
            PolicyDecision::NeedsConfirmation { .. } => "NEEDS CONFIRMATION",
            _ => "UNKNOWN",
        };
        println!("  {tool:20} → {status}");
    }

    // ── 3. Wire policies into AgentConfig ───────────────────────────────

    let config = AgentConfig::builder()
        .policies([
            PolicyRule::allow("view_file"),
            PolicyRule::allow("list_directory"),
            PolicyRule::deny("run_command"),
            PolicyRule::deny("edit_file"),
            PolicyRule::DenyAll,
        ])
        .build();

    let bridge = AgyBridge::builder().build()?;
    let agent = bridge.agent(config).await?;

    let prompt = "List the files in the current directory.";
    println!("  User: {prompt}");
    let response_text = agent.chat(prompt).await?.text().await?;
    println!("  Agent: {response_text}");

    agent.shutdown().await?;
    Ok(())
}

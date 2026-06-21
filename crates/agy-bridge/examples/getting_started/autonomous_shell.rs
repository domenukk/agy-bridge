//! Demonstrates an agent with shell execution capabilities.
//!
//! By enabling specific built-in tools (here `RunCommand`), the agent
//! gains the ability to execute shell commands on the host.  A workspace
//! is configured so the agent can list its contents.
//!
//! # ⚠️ Security Warning
//!
//! This example uses a restricted policy set that only allows `run_command`
//! within a workspace-scoped directory. For production use, always:
//! - Use [`agy_bridge::policies::safe_defaults()`] or a custom restrictive policy.
//! - Never use `PolicyRule::AllowAll` in production — it grants unrestricted
//!   access to all tools without user confirmation.
//! - Scope workspaces to the narrowest directory possible.

use agy_bridge::{
    AgyBridge, PolicyRule,
    config::{AgentConfig, BuiltinTools, CapabilitiesConfig},
    policies::confirm_run_command,
};

/// ⚠️ SECURITY WARNING: This example grants shell access to the LLM agent.
/// In production, always use restricted policies and user confirmation.
#[tokio::main]
async fn main() -> Result<(), agy_bridge::error::Error> {
    agy_bridge::load_dotenv();
    let bridge = AgyBridge::builder().build()?;

    let cwd = std::env::current_dir()?;

    // Enable only shell-related tools — no file editing or subagents.
    let config = AgentConfig::builder()
        .capabilities(CapabilitiesConfig::with_tools(vec![
            BuiltinTools::RunCommand,
            BuiltinTools::Finish,
        ]))
        .workspaces(&[cwd])
        // ⚠️ Use restricted policies: confirm before running shell commands,
        // deny everything else by default.
        .policies([confirm_run_command(), PolicyRule::DenyAll])
        .build();

    let agent = bridge.agent(config).await?;

    let prompt = "List the files in the current directory using a shell command.";
    println!("User: {prompt}");
    let text = agent.chat(prompt).await?.text().await?;
    println!("Agent: {text}");

    agent.shutdown().await?;
    Ok(())
}

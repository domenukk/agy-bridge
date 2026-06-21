//! Doc maintenance agent — audits and fixes Markdown documentation.
//!
//! Demonstrates:
//! - Hook-based tool call logging and file-type policy enforcement
//! - `Hooks` for callback storage + `HookEntry` config for the builder
//! - Workspace-scoped agent restricted to `.md` files

use agy_bridge::{config::CapabilitiesConfig, hooks::HookResult, prelude::*};

/// Extract the file/directory path from a tool-call's arguments.
fn extract_path_arg(args: &serde_json::Value) -> String {
    let Some(obj) = args.as_object() else {
        return String::new();
    };
    let raw = obj
        .get("file_path")
        .or_else(|| obj.get("path"))
        .or_else(|| obj.get("directory_path"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    raw.strip_prefix("file://").unwrap_or(raw).to_string()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    agy_bridge::load_dotenv();
    println!("Creating Doc Maintenance Agent...\n");
    let bridge = AgyBridge::builder().build()?;

    let target_dir = std::env::current_dir()?;
    let target_dir_str = target_dir.to_string_lossy().to_string();

    // Register the callback on the runner and the matching entry for config.
    let mut hook_runner = Hooks::new();
    hook_runner.on_pre_tool_call_decide("md_policy", move |ctx| {
        let path = extract_path_arg(&ctx.tool_args);

        // Log every tool invocation.
        if path.is_empty() {
            println!("  [{}] args: {}", ctx.tool_name, ctx.tool_args);
        } else {
            println!("  [{}] {path}", ctx.tool_name);
        }

        // Only allow editing Markdown files within the target directory.
        if ctx.tool_name == "edit_file" {
            let is_md = std::path::Path::new(&path)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"));
            if is_md && path.starts_with(&target_dir_str) {
                HookResult::allow()
            } else {
                HookResult::deny("Only .md files in the target directory may be edited.")
            }
        } else {
            HookResult::allow()
        }
    });

    let system_instructions = format!(
        "You are an expert Technical Writer for the Google Antigravity SDK.\n\
         Audit all Markdown documentation in the target directory and fix \
         any discrepancies with the code.\n\
         You may ONLY edit .md files within: {}",
        target_dir.display()
    );

    let config = AgentConfig::builder()
        .system_instructions(system_instructions)
        .workspaces(std::slice::from_ref(&target_dir))
        .capabilities(CapabilitiesConfig::default())
        .policies(vec![PolicyRule::AllowAll])
        .build();

    let agent = bridge.agent(config).hooks(hook_runner).await?;

    let prompt = "Check all documentation in the target directory and fix any discrepancies.";
    println!("Prompt: {prompt}\n");
    let response_text = agent.chat(prompt).await?.text().await?;
    println!("{response_text}");

    agent.shutdown().await?;
    Ok(())
}

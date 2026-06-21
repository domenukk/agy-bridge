//! Docstring maintenance agent — audits and fixes Python docstrings.
//!
//! Demonstrates:
//! - A different policy than `doc_maintenance_agent` (Python files only)
//! - Post-tool-call summary hook for edit tracking
//! - `Hooks` for callback storage + `HookEntry` config for the builder

use std::sync::{Arc, Mutex};

use agy_bridge::{config::CapabilitiesConfig, hooks::HookResult, prelude::*};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    agy_bridge::load_dotenv();
    println!("Creating Docstring Maintenance Agent...\n");
    let bridge = AgyBridge::builder().build()?;

    let target_dir = std::env::current_dir()?;
    let target_dir_str = target_dir.to_string_lossy().to_string();

    // Track which files were edited for the summary.
    let edited_files: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let edited_clone = Arc::clone(&edited_files);

    let mut hook_runner = Hooks::new();

    // Policy: only allow editing .py files within the target directory.
    hook_runner.on_pre_tool_call_decide("py_policy", move |ctx| {
        if ctx.tool_name != "edit_file" {
            return HookResult::allow();
        }
        let path = ctx
            .tool_args
            .as_object()
            .and_then(|o| o.get("file_path").or_else(|| o.get("path")))
            .and_then(serde_json::Value::as_str)
            .map_or("", |s| s.strip_prefix("file://").unwrap_or(s));

        let is_py = std::path::Path::new(path)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("py"));

        if is_py && path.starts_with(&target_dir_str) {
            HookResult::allow()
        } else {
            HookResult::deny("Only .py files in the target directory may be edited.")
        }
    });

    // Track every successful edit for the summary report.
    hook_runner.on_post_tool_call("edit_tracker", move |ctx| {
        if ctx.tool_name == "edit_file" {
            let path = ctx
                .tool_args
                .as_object()
                .and_then(|o| o.get("file_path").or_else(|| o.get("path")))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("<unknown>")
                .to_string();
            println!("  ✏️  Edited: {path}");
            edited_clone.lock().unwrap().push(path);
        }
    });

    let system_instructions = format!(
        "You are an expert Docstring Maintenance Agent.\n\
         Audit all Python files in the target directory and ensure all public \
         symbols have Google-style docstrings. Add or update docstrings as needed.\n\
         Do NOT modify implementation code — only docstring blocks.\n\
         Target directory: {}",
        target_dir.display()
    );

    let config = AgentConfig::builder()
        .system_instructions(system_instructions)
        .workspaces(std::slice::from_ref(&target_dir))
        .capabilities(CapabilitiesConfig::default())
        .policies(vec![PolicyRule::AllowAll])
        .build();

    let agent = bridge.agent(config).hooks(hook_runner).await?;

    let prompt = "Audit all Python files and ensure public symbols have Google-style docstrings.";
    println!("Prompt: {prompt}\n");
    let response_text = agent.chat(prompt).await?.text().await?;
    println!("{response_text}");

    // Print edit summary — drop the lock before the async shutdown.
    {
        let files = edited_files.lock().unwrap();
        println!("\n--- Edit Summary: {} files modified ---", files.len());
        for f in files.iter() {
            println!("  • {f}");
        }
    }

    agent.shutdown().await?;
    Ok(())
}

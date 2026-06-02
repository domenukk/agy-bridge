//! Live integration tests for agy-bridge hooks against the real Gemini backend.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use agy_bridge::{
    AgyBridge,
    config::{AgentConfig, BuiltinTools, CapabilitiesConfig},
    hooks::{HookResult, Hooks},
};

mod common;

fn api_key() -> String {
    common::api_key()
}

fn test_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
}

#[test]
fn test_hooks_lifecycle_live() {
    common::run_live_test("test_hooks_lifecycle_live", || {
        let rt = test_runtime();
        rt.block_on(async {
            let key = api_key();

            let events = Arc::new(Mutex::new(Vec::new()));

            let mut hook_runner = Hooks::new();

            let e1 = Arc::clone(&events);
            hook_runner.on_pre_turn("log_pre_turn", move |ctx| {
                e1.lock().unwrap().push(format!("pre_turn: {}", ctx.prompt));
            });

            let e2 = Arc::clone(&events);
            hook_runner.on_post_turn("log_post_turn", move |ctx| {
                e2.lock().unwrap().push(format!(
                    "post_turn: {}",
                    ctx.response_text.chars().take(20).collect::<String>()
                ));
            });

            let e3 = Arc::clone(&events);
            hook_runner.on_pre_tool_call_decide("log_pre_tool", move |ctx| {
                e3.lock()
                    .unwrap()
                    .push(format!("pre_tool: {}", ctx.tool_name));
                HookResult::allow()
            });

            let e4 = Arc::clone(&events);
            hook_runner.on_post_tool_call("log_post_tool", move |ctx| {
                e4.lock()
                    .unwrap()
                    .push(format!("post_tool: {}", ctx.tool_name));
            });

            let config = AgentConfig::builder()
                .api_key(&key)
                .model("gemini-3.5-flash")
                .capabilities(CapabilitiesConfig::with_tools(vec![BuiltinTools::ViewFile]))
                .build();

            let bridge = AgyBridge::builder()
                .chat_timeout(Duration::from_mins(1))
                .build()
                .unwrap();

            let agent = bridge.agent(config).hooks(hook_runner).await.unwrap();

            let cargo_toml = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
            let prompt = format!(
                "Use view_file to read the file '{}' and tell me its contents.",
                cargo_toml.display(),
            );
            let text = agent.chat(&*prompt).await.unwrap().text().await.unwrap();
            println!("AGENT FULL RESPONSE: {text}");

            tokio::time::sleep(Duration::from_secs(3)).await;
            agent.shutdown().await.unwrap();

            let events_list = events.lock().unwrap().clone();

            // Assert we saw pre_turn
            assert!(
                events_list
                    .iter()
                    .any(|e| e.starts_with("pre_turn:") && e.contains("view_file")),
                "Expected pre_turn event mentioning view_file, got: {events_list:?}"
            );

            // Assert we saw post_turn
            assert!(
                events_list.iter().any(|e| e.starts_with("post_turn:")),
                "Expected post_turn event, got: {events_list:?}"
            );

            // Assert ordering: pre_turn always comes before post_turn
            let pre_turn_idx = events_list
                .iter()
                .position(|e| e.starts_with("pre_turn:"))
                .unwrap();
            let post_turn_idx = events_list
                .iter()
                .position(|e| e.starts_with("post_turn:"))
                .unwrap();
            assert!(pre_turn_idx < post_turn_idx);
        });
    });
}

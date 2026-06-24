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
                .chat_timeout(Duration::from_mins(3))
                .build()?;

            let agent = bridge.agent(config).hooks(hook_runner).await?;

            let cargo_toml = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
            let prompt = format!(
                "Use view_file to read the file '{}' and tell me its contents.",
                cargo_toml.display(),
            );
            let text = agent.chat(&*prompt).await?.text().await?;
            println!("AGENT FULL RESPONSE: {text}");

            let mut post_turn_seen = false;
            for _ in 0..300 {
                tokio::time::sleep(Duration::from_millis(100)).await;
                if events
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|e| e.starts_with("post_turn:"))
                {
                    post_turn_seen = true;
                    break;
                }
            }
            assert!(
                post_turn_seen,
                "Expected post_turn event, got: {:?}",
                events.lock().unwrap()
            );

            agent.shutdown().await?;

            let events_list = events.lock().unwrap().clone();

            // Assert we saw pre_turn
            assert!(
                events_list
                    .iter()
                    .any(|e| e.starts_with("pre_turn:") && e.contains("view_file")),
                "Expected pre_turn event mentioning view_file, got: {events_list:?}"
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

            Ok(())
        })
    });
}

#[test]
fn test_on_tool_error_hook_live() {
    /// A tool that always returns an error, used to trigger `on_tool_error`.
    struct AlwaysFails;

    /// Empty parameters for [`AlwaysFails`].
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    struct AlwaysFailsParams {}

    impl agy_bridge::tools::RustTool for AlwaysFails {
        type Params = AlwaysFailsParams;
        const NAME: &'static str = "always_fails";
        const DESCRIPTION: &'static str =
            "A tool that always returns an error. Call it with no arguments.";

        async fn call(
            &self,
            _params: Self::Params,
            _ctx: &agy_bridge::tools::ToolContext,
        ) -> Result<agy_bridge::tools::ToolOutput, agy_bridge::tools::ToolError> {
            Err(agy_bridge::tools::ToolError::new("intentional test error"))
        }
    }

    common::run_live_test("test_on_tool_error_hook_live", || {
        let rt = test_runtime();
        rt.block_on(async {
            let key = api_key();

            let events = Arc::new(Mutex::new(Vec::new()));

            let mut hooks = Hooks::new();

            let e = Arc::clone(&events);
            hooks.on_tool_error("log_tool_error", move |ctx| {
                e.lock()
                    .unwrap()
                    .push(format!("tool_error:{}:{}", ctx.tool_name, ctx.error));
            });

            // Also register pre_tool_call_decide to allow all tool calls.
            hooks.on_pre_tool_call_decide("allow_all", |_ctx| HookResult::allow());

            // Register a custom tool that always errors.
            let mut registry = agy_bridge::tools::ToolRegistry::new();
            registry.register(AlwaysFails);

            let config = AgentConfig::builder()
                .api_key(&key)
                .model("gemini-3.5-flash")
                .build();

            let bridge = AgyBridge::builder()
                .chat_timeout(Duration::from_mins(3))
                .build()?;

            let agent = bridge.agent(config).hooks(hooks).tools(registry).await?;

            let prompt = "Call the always_fails tool now. Report what happened.";
            let _text = agent.chat(prompt).await?.text().await?;

            // Poll for the tool_error event.
            let mut seen = false;
            for _ in 0..300 {
                tokio::time::sleep(Duration::from_millis(100)).await;
                if events
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|e| e.starts_with("tool_error:"))
                {
                    seen = true;
                    break;
                }
            }

            agent.shutdown().await?;

            assert!(
                seen,
                "Expected on_tool_error event to fire, got: {:?}",
                events.lock().unwrap()
            );

            Ok(())
        })
    });
}

#[test]
fn test_transform_tool_input_hook_live() {
    common::run_live_test("test_transform_tool_input_hook_live", || {
        let rt = test_runtime();
        rt.block_on(async {
            let key = api_key();

            let events = Arc::new(Mutex::new(Vec::new()));

            let mut hooks = Hooks::new();

            // Register a transform that records when it runs.
            let e = Arc::clone(&events);
            hooks.on_transform_tool_input("log_transform", move |ctx| {
                e.lock()
                    .unwrap()
                    .push(format!("transform:{}", ctx.tool_name));
                // Return None to leave args unchanged.
                None
            });

            hooks.on_pre_tool_call_decide("allow_all", |_ctx| HookResult::allow());

            let config = AgentConfig::builder()
                .api_key(&key)
                .model("gemini-3.5-flash")
                .capabilities(CapabilitiesConfig::with_tools(vec![BuiltinTools::ViewFile]))
                .build();

            let bridge = AgyBridge::builder()
                .chat_timeout(Duration::from_mins(3))
                .build()?;

            let agent = bridge.agent(config).hooks(hooks).await?;

            let cargo_toml = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
            let prompt = format!(
                "Use view_file to read '{}' and tell me the package name.",
                cargo_toml.display(),
            );
            let _text = agent.chat(&*prompt).await?.text().await?;

            // Poll for the transform event.
            let mut seen = false;
            for _ in 0..300 {
                tokio::time::sleep(Duration::from_millis(100)).await;
                if events
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|e| e.starts_with("transform:"))
                {
                    seen = true;
                    break;
                }
            }

            agent.shutdown().await?;

            assert!(
                seen,
                "Expected transform_tool_input event to fire, got: {:?}",
                events.lock().unwrap()
            );

            Ok(())
        })
    });
}

#[test]
fn test_session_and_interaction_hooks_live() {
    common::run_live_test("test_session_and_interaction_hooks_live", || {
        let rt = test_runtime();
        rt.block_on(async {
            let key = api_key();

            let events = Arc::new(Mutex::new(Vec::new()));

            let mut hooks = Hooks::new();

            let e1 = Arc::clone(&events);
            hooks.on_session_start("log_session_start", move |ctx| {
                e1.lock()
                    .unwrap()
                    .push(format!("session_start:{}", ctx.session.session_id));
            });

            let e2 = Arc::clone(&events);
            hooks.on_session_end("log_session_end", move |ctx| {
                e2.lock()
                    .unwrap()
                    .push(format!("session_end:{}", ctx.session.session_id));
            });

            let e3 = Arc::clone(&events);
            hooks.on_interaction("log_interaction", move |ctx| {
                e3.lock()
                    .unwrap()
                    .push(format!("interaction:{}", ctx.message.chars().take(30).collect::<String>()));
                HookResult::allow()
            });

            let config = AgentConfig::builder()
                .api_key(&key)
                .model("gemini-3.5-flash")
                .build();

            let bridge = AgyBridge::builder()
                .chat_timeout(Duration::from_mins(3))
                .build()?;

            let agent = bridge.agent(config).hooks(hooks).await?;

            // Simple chat to trigger interaction hook.
            let _text = agent.chat("Say 'hello' and nothing else.").await?.text().await?;

            // Poll for session_start (fires on agent creation).
            let mut start_seen = false;
            for _ in 0..300 {
                tokio::time::sleep(Duration::from_millis(100)).await;
                if events
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|e| e.starts_with("session_start:"))
                {
                    start_seen = true;
                    break;
                }
            }

            agent.shutdown().await?;

            // After shutdown, poll for session_end.
            let mut end_seen = false;
            for _ in 0..100 {
                tokio::time::sleep(Duration::from_millis(100)).await;
                if events
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|e| e.starts_with("session_end:"))
                {
                    end_seen = true;
                    break;
                }
            }

            let events_list = events.lock().unwrap().clone();

            assert!(
                start_seen,
                "Expected session_start event, got: {events_list:?}"
            );
            assert!(
                end_seen,
                "Expected session_end event, got: {events_list:?}"
            );

            // Verify ordering: session_start before session_end.
            if let (Some(start_idx), Some(end_idx)) = (
                events_list
                    .iter()
                    .position(|e| e.starts_with("session_start:")),
                events_list
                    .iter()
                    .position(|e| e.starts_with("session_end:")),
            ) {
                assert!(
                    start_idx < end_idx,
                    "session_start should precede session_end, got indices: start={start_idx}, end={end_idx}"
                );
            }

            Ok(())
        })
    });
}

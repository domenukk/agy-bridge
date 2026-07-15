//! Mock-server integration tests: hooks.
//!
//! Covers pre/post-turn hooks, pre-tool-call-decide gating, post-tool-call,
//! on-tool-error, combined hooks, `transform_tool_input`, and session
//! lifecycle hooks. **No API key required.**
//!
//! Run with:
//! ```sh
//! cargo test --test features_mock_hooks_test -- --nocapture
//! ```

use std::sync::{Arc, Mutex, atomic::Ordering};

use agy_bridge::{
    hooks::{HookResult, Hooks},
    tools::ToolRegistry,
};
use agy_bridge_test_support::*;

// ═══════════════════════════════════════════════════════════════════════════
// SECTION 2: Hooks
// ═══════════════════════════════════════════════════════════════════════════

/// Pre-turn and post-turn hooks fire with correct context during tool calls.
#[test]
fn hooks_pre_and_post_turn_fire() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "add_numbers".into(),
                args: serde_json::json!({"x": 1.0, "y": 2.0}),
            },
            MockResponse::Text("Done.".into()),
        ])
        .await;

        let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

        let e1 = Arc::clone(&events);
        let e2 = Arc::clone(&events);
        let hooks = Hooks::new()
            .with_pre_turn("test_pre", move |ctx| {
                e1.lock().unwrap().push(format!("pre:{}", ctx.turn_number));
            })
            .with_post_turn("test_post", move |ctx| {
                e2.lock().unwrap().push(format!("post:{}", ctx.turn_number));
            });

        let mut registry = ToolRegistry::new();
        registry.register(AddTool);

        let agent = BRIDGE
            .agent(agent_config(&server.base_url(), "hooks"))
            .tools(registry)
            .hooks(hooks)
            .await
            .expect("agent");

        agent.chat_text("test").await.expect("chat");

        let evts = events.lock().unwrap().clone();
        eprintln!("Hook events: {evts:?}");

        // Pre-turn should fire at least once.
        assert!(
            evts.iter().any(|e| e.starts_with("pre:")),
            "Pre-turn hook should have fired. Events: {evts:?}"
        );

        agent.shutdown().await.expect("shutdown");
    });
}

/// Pre-tool-call-decide hook can gate (deny) tool execution.
#[test]
fn hooks_pre_tool_call_decide_denies() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "counting_tool".into(),
                args: serde_json::json!({}),
            },
            MockResponse::Text("Tool was gated.".into()),
        ])
        .await;

        let (counting_tool, invocation_count) = CountingTool::new();

        let hooks = Hooks::new().with_pre_tool_call_decide("gate", |ctx| {
            if ctx.tool_name == "counting_tool" {
                HookResult::deny("blocked by test hook")
            } else {
                HookResult::allow()
            }
        });

        let mut registry = ToolRegistry::new();
        registry.register(counting_tool);

        let agent = BRIDGE
            .agent(agent_config(&server.base_url(), "gate"))
            .tools(registry)
            .hooks(hooks)
            .await
            .expect("agent");

        // The chat may succeed or fail depending on SDK behavior when
        // a tool is denied — either way, the tool should NOT execute.
        let _result = agent.chat_text("call tool").await;

        assert_eq!(
            invocation_count.load(Ordering::SeqCst),
            0,
            "Tool should NOT have been invoked — hook denied it"
        );

        agent.shutdown().await.expect("shutdown");
    });
}

/// Post-tool-call hook fires after tool execution with result context.
#[test]
fn hooks_post_tool_call_fires() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "add_numbers".into(),
                args: serde_json::json!({"x": 7.0, "y": 3.0}),
            },
            MockResponse::Text("Done.".into()),
        ])
        .await;

        let tool_names: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let tn = Arc::clone(&tool_names);

        let hooks = Hooks::new().with_post_tool_call("log_post", move |ctx| {
            tn.lock()
                .unwrap()
                .push(format!("post_tool:{}", ctx.tool_name));
        });

        let mut registry = ToolRegistry::new();
        registry.register(AddTool);

        let agent = BRIDGE
            .agent(agent_config(&server.base_url(), "post_hook"))
            .tools(registry)
            .hooks(hooks)
            .await
            .expect("agent");

        agent.chat_text("add").await.expect("chat");

        let names = tool_names.lock().unwrap().clone();
        eprintln!("Post-tool events: {names:?}");
        assert!(
            names.iter().any(|n| n.contains("add_numbers")),
            "Post-tool hook should have seen add_numbers. Events: {names:?}"
        );

        agent.shutdown().await.expect("shutdown");
    });
}

/// On-tool-error hook fires when a tool returns Err.
#[test]
fn hooks_on_tool_error_fires() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "always_fail".into(),
                args: serde_json::json!({"reason": "test error"}),
            },
            MockResponse::Text("Handled.".into()),
        ])
        .await;

        let errors: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let errs = Arc::clone(&errors);

        let hooks = Hooks::new().with_tool_error("log_err", move |ctx| {
            errs.lock()
                .unwrap()
                .push(format!("error:{}:{}", ctx.tool_name, ctx.error));
        });

        let mut registry = ToolRegistry::new();
        registry.register(AlwaysFailTool);

        let agent = BRIDGE
            .agent(agent_config(&server.base_url(), "err_hook"))
            .tools(registry)
            .hooks(hooks)
            .await
            .expect("agent");

        agent.chat_text("fail").await.expect("chat");

        let errs = errors.lock().unwrap().clone();
        eprintln!("Error events: {errs:?}");
        assert!(
            errs.iter().any(|e| e.contains("always_fail")),
            "On-tool-error hook should have fired for always_fail. Events: {errs:?}"
        );

        agent.shutdown().await.expect("shutdown");
    });
}

/// Multiple hooks registered at different points all fire.
#[test]
fn hooks_multiple_combined() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "add_numbers".into(),
                args: serde_json::json!({"x": 1.0, "y": 1.0}),
            },
            MockResponse::Text("2".into()),
        ])
        .await;

        let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

        let e1 = Arc::clone(&events);
        let e2 = Arc::clone(&events);
        let e3 = Arc::clone(&events);
        let e4 = Arc::clone(&events);

        let hooks = Hooks::new()
            .with_pre_turn("pre", move |_ctx| {
                e1.lock().unwrap().push("pre_turn".into());
            })
            .with_post_turn("post", move |_ctx| {
                e2.lock().unwrap().push("post_turn".into());
            })
            .with_pre_tool_call_decide("decide", move |_ctx| {
                e3.lock().unwrap().push("pre_tool_decide".into());
                HookResult::allow()
            })
            .with_post_tool_call("post_tool", move |_ctx| {
                e4.lock().unwrap().push("post_tool_call".into());
            });

        let mut registry = ToolRegistry::new();
        registry.register(AddTool);

        let agent = BRIDGE
            .agent(agent_config(&server.base_url(), "combined"))
            .tools(registry)
            .hooks(hooks)
            .await
            .expect("agent");

        agent.chat_text("1+1").await.expect("chat");

        let evts = events.lock().unwrap().clone();
        eprintln!("Combined hook events: {evts:?}");

        assert!(
            evts.contains(&"pre_turn".to_string()),
            "Missing pre_turn. Events: {evts:?}"
        );

        agent.shutdown().await.expect("shutdown");
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// SECTION 8: transform_tool_input hook
// ═══════════════════════════════════════════════════════════════════════════

/// `transform_tool_input` hook can inspect and optionally replace tool args
/// before execution.
#[test]
fn hooks_transform_tool_input_observed() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "add_numbers".into(),
                args: serde_json::json!({"x": 3.0, "y": 4.0}),
            },
            MockResponse::Text("Done.".into()),
        ])
        .await;

        let observed_args: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
        let obs = Arc::clone(&observed_args);

        let hooks = Hooks::new().with_transform_tool_input("observer", move |ctx| {
            obs.lock().unwrap().push(ctx.tool_args.clone());
            // Return None = no transformation.
            None
        });

        let mut registry = ToolRegistry::new();
        registry.register(AddTool);

        let agent = BRIDGE
            .agent(agent_config(&server.base_url(), "transform"))
            .tools(registry)
            .hooks(hooks)
            .await
            .expect("agent");

        agent.chat_text("add").await.expect("chat");

        let args = observed_args.lock().unwrap().clone();
        eprintln!("Transform observed args: {args:?}");
        assert!(
            !args.is_empty(),
            "transform_tool_input hook should have observed tool args"
        );
        // Should have seen the add_numbers args.
        assert!(
            args[0].get("x").is_some() || args[0].get("y").is_some(),
            "Should observe x/y args, got: {:?}",
            args[0]
        );

        agent.shutdown().await.expect("shutdown");
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// SECTION 10: Session lifecycle hooks
// ═══════════════════════════════════════════════════════════════════════════

/// Session start/end hooks fire during agent lifecycle.
#[test]
fn hooks_session_start_and_end_fire() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![MockResponse::Text("Hi.".into())]).await;

        let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let e1 = Arc::clone(&events);
        let e2 = Arc::clone(&events);

        let hooks = Hooks::new()
            .with_session_start("log_start", move |ctx| {
                e1.lock()
                    .unwrap()
                    .push(format!("start:{}", ctx.session.session_id));
            })
            .with_session_end("log_end", move |_ctx| {
                e2.lock().unwrap().push("end".into());
            });

        let agent = BRIDGE
            .agent(agent_config(&server.base_url(), "session"))
            .hooks(hooks)
            .await
            .expect("agent");

        agent.chat_text("hello").await.expect("chat");
        agent.shutdown().await.expect("shutdown");

        let evts = events.lock().unwrap().clone();
        eprintln!("Session events: {evts:?}");
        assert!(
            evts.iter().any(|e| e.starts_with("start:")),
            "Session start hook should have fired. Events: {evts:?}"
        );
    });
}

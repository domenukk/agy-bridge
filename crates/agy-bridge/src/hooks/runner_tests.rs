use std::time::SystemTime;

use super::{super::types::SessionContext, *};

#[test]
fn hook_runner_pre_turn_callback_fires() {
    use std::sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    };

    let counter = Arc::new(AtomicU32::new(0));
    let c = Arc::clone(&counter);

    let mut runner = Hooks::new();
    runner.register(
        "counter",
        HookCallback::PreTurn(Box::new(move |ctx| {
            c.fetch_add(ctx.turn_number, Ordering::SeqCst);
        })),
    );

    runner.run_pre_turn(&PreTurnContext {
        prompt: "hello".into(),
        turn_number: 7,
    });
    assert_eq!(counter.load(Ordering::SeqCst), 7);
}

#[test]
fn hook_runner_pre_tool_call_decide_deny_short_circuits() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    let mut runner = Hooks::new();

    // First callback: allow
    runner.register(
        "allow_gate",
        HookCallback::PreToolCallDecide(Box::new(|_ctx| HookResult::allow())),
    );

    // Second callback: deny
    runner.register(
        "deny_gate",
        HookCallback::PreToolCallDecide(Box::new(|_ctx| HookResult::deny("blocked"))),
    );

    // Third callback: should never be reached
    let reached = Arc::new(AtomicBool::new(false));
    let r = Arc::clone(&reached);
    runner.register(
        "unreachable",
        HookCallback::PreToolCallDecide(Box::new(move |_ctx| {
            r.store(true, Ordering::SeqCst);
            HookResult::allow()
        })),
    );

    let result = runner.run_pre_tool_call_decide(&PreToolCallDecideContext {
        tool_name: "some_tool".into(),
        tool_args: serde_json::Value::Null,
    });

    assert!(!result.allow);
    assert_eq!(result.message, "blocked");
    assert!(
        !reached.load(Ordering::SeqCst),
        "third callback should not fire after deny"
    );
}

#[test]
fn hook_runner_multiple_callbacks_fire_in_order() {
    use std::sync::{Arc, Mutex};

    let log = Arc::new(Mutex::new(Vec::<String>::new()));

    let mut runner = Hooks::new();
    for i in 0..3 {
        let l = Arc::clone(&log);
        runner.register(
            format!("hook_{i}"),
            HookCallback::PreTurn(Box::new(move |_ctx| {
                l.lock().unwrap().push(format!("hook_{i}"));
            })),
        );
    }

    runner.run_pre_turn(&PreTurnContext {
        prompt: "test".into(),
        turn_number: 1,
    });

    let entries = log.lock().unwrap().clone();
    assert_eq!(entries, vec!["hook_0", "hook_1", "hook_2"]);
}

#[test]
fn hook_runner_post_tool_call_receives_result() {
    use std::sync::{Arc, Mutex};

    let captured = Arc::new(Mutex::new(String::new()));
    let c = Arc::clone(&captured);

    let mut runner = Hooks::new();
    runner.register(
        "capture",
        HookCallback::PostToolCall(Box::new(move |ctx| {
            *c.lock().unwrap() = ctx.result.clone();
        })),
    );

    runner.run_post_tool_call(&PostToolCallContext {
        tool_name: "read_file".into(),
        tool_args: serde_json::json!({"path": "/tmp/x"}),
        result: "file contents here".into(),
        metadata: serde_json::Value::Null,
    });

    assert_eq!(*captured.lock().unwrap(), "file contents here");
}

#[test]
fn hook_runner_on_tool_error_fires_with_context() {
    use std::sync::{Arc, Mutex};

    let captured_error = Arc::new(Mutex::new(String::new()));
    let captured_tool = Arc::new(Mutex::new(String::new()));
    let ce = Arc::clone(&captured_error);
    let ct = Arc::clone(&captured_tool);

    let mut runner = Hooks::new();
    runner.register(
        "error_logger",
        HookCallback::OnToolError(Box::new(move |ctx| {
            *ce.lock().unwrap() = ctx.error.clone();
            *ct.lock().unwrap() = ctx.tool_name.clone();
        })),
    );

    runner.run_on_tool_error(&OnToolErrorContext {
        tool_name: "write_file".into(),
        tool_args: serde_json::json!({}),
        error: "permission denied".into(),
        metadata: serde_json::Value::Null,
    });

    assert_eq!(*captured_error.lock().unwrap(), "permission denied");
    assert_eq!(*captured_tool.lock().unwrap(), "write_file");
}

#[test]
fn hook_runner_default_is_empty() {
    let runner = Hooks::default();
    runner.run_pre_turn(&PreTurnContext {
        prompt: "x".into(),
        turn_number: 1,
    });
    let result = runner.run_pre_tool_call_decide(&PreToolCallDecideContext {
        tool_name: "t".into(),
        tool_args: serde_json::Value::Null,
    });
    assert!(result.allow, "empty runner should allow everything");
}

#[test]
fn hook_callback_debug_format() {
    // NOLINT: |_| closure arg intentionally unused — mock callback for Debug format test
    let cb = HookCallback::PreTurn(Box::new(|_| {}));
    let dbg = format!("{cb:?}");
    assert_eq!(dbg, "HookCallback::pre_turn");
}

// ── Panic recovery tests ────────────────────────────────────────────

#[test]
fn hook_runner_pre_turn_panic_recovery() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    let reached = Arc::new(AtomicBool::new(false));
    let r = Arc::clone(&reached);

    let mut runner = Hooks::new();
    // Register a hook that panics.
    runner.register(
        "panicker",
        HookCallback::PreTurn(Box::new(|_ctx| {
            panic!("intentional test panic in pre_turn hook");
        })),
    );
    // Register a second hook after the panicking one — it should still fire.
    runner.register(
        "after_panic",
        HookCallback::PreTurn(Box::new(move |_ctx| {
            r.store(true, Ordering::SeqCst);
        })),
    );

    // run_pre_turn should NOT propagate the panic.
    runner.run_pre_turn(&PreTurnContext {
        prompt: "test".into(),
        turn_number: 1,
    });

    assert!(
        reached.load(Ordering::SeqCst),
        "second hook should fire even after the first panicked"
    );
}

#[test]
fn hook_runner_pre_tool_call_decide_panic_returns_deny() {
    let mut runner = Hooks::new();
    // Register a hook that panics.
    runner.register(
        "panicker",
        HookCallback::PreToolCallDecide(Box::new(|_ctx| {
            panic!("intentional test panic in pre_tool_call_decide");
        })),
    );

    let result = runner.run_pre_tool_call_decide(&PreToolCallDecideContext {
        tool_name: "dangerous_tool".into(),
        tool_args: serde_json::Value::Null,
    });

    // A panicking PreToolCallDecide hook should deny the tool call as a safe default.
    assert!(!result.allow, "panicking hook should deny the tool call");
    assert!(
        result.message.contains("panicked"),
        "deny message should mention the panic: {:?}",
        result.message
    );
}

// ── Multiple callbacks at same point in Hooks ──────────────────

#[test]
fn hook_runner_multiple_callbacks_at_same_point() {
    use std::sync::{Arc, Mutex};

    let log = Arc::new(Mutex::new(Vec::<String>::new()));

    let mut runner = Hooks::new();
    for i in 0..5 {
        let l = Arc::clone(&log);
        runner.register(
            format!("post_turn_{i}"),
            HookCallback::PostTurn(Box::new(move |_ctx| {
                l.lock().unwrap().push(format!("post_turn_{i}"));
            })),
        );
    }

    runner.run_post_turn(&PostTurnContext {
        response_text: "response".into(),
        turn_number: 1,
    });

    let entries = log.lock().unwrap().clone();
    assert_eq!(
        entries,
        vec![
            "post_turn_0",
            "post_turn_1",
            "post_turn_2",
            "post_turn_3",
            "post_turn_4"
        ],
        "all 5 callbacks should fire in registration order"
    );
}

// ── Duplicate hook replacement in Hooks ────────────────────────

#[test]
fn hook_runner_duplicate_replaces_previous() {
    use std::sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    };

    let counter = Arc::new(AtomicU32::new(0));

    let mut runner = Hooks::new();

    // Register a hook that adds 10.
    let c1 = Arc::clone(&counter);
    runner.register(
        "counter_hook",
        HookCallback::PreTurn(Box::new(move |_ctx| {
            c1.fetch_add(10, Ordering::SeqCst);
        })),
    );

    // Register a hook with the SAME name and SAME point — should replace.
    let c2 = Arc::clone(&counter);
    runner.register(
        "counter_hook",
        HookCallback::PreTurn(Box::new(move |_ctx| {
            c2.fetch_add(1, Ordering::SeqCst);
        })),
    );

    runner.run_pre_turn(&PreTurnContext {
        prompt: "test".into(),
        turn_number: 1,
    });

    // Only the replacement (adds 1) should have run, not the original (adds 10).
    let value = counter.load(Ordering::SeqCst);
    assert_eq!(
        value, 1,
        "duplicate hook should replace the previous; expected 1 but got {value}"
    );
}

// ── Convenience builder tests ───────────────────────────────────────

#[test]
fn convenience_on_pre_turn() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    let fired = Arc::new(AtomicBool::new(false));
    let f = Arc::clone(&fired);

    let mut runner = Hooks::new();
    runner.on_pre_turn("test", move |_ctx| {
        f.store(true, Ordering::SeqCst);
    });
    runner.run_pre_turn(&PreTurnContext {
        prompt: "hi".into(),
        turn_number: 1,
    });
    assert!(fired.load(Ordering::SeqCst));
}

#[test]
fn convenience_on_post_turn() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    let fired = Arc::new(AtomicBool::new(false));
    let f = Arc::clone(&fired);

    let mut runner = Hooks::new();
    runner.on_post_turn("test", move |_ctx| {
        f.store(true, Ordering::SeqCst);
    });
    runner.run_post_turn(&PostTurnContext {
        response_text: "ok".into(),
        turn_number: 1,
    });
    assert!(fired.load(Ordering::SeqCst));
}

#[test]
fn convenience_on_pre_tool_call_decide() {
    let mut runner = Hooks::new();
    runner.on_pre_tool_call_decide("gate", |ctx| {
        if ctx.tool_name == "blocked" {
            HookResult::deny("nope")
        } else {
            HookResult::allow()
        }
    });

    let allowed = runner.run_pre_tool_call_decide(&PreToolCallDecideContext {
        tool_name: "ok_tool".into(),
        tool_args: serde_json::Value::Null,
    });
    assert!(allowed.allow);

    let denied = runner.run_pre_tool_call_decide(&PreToolCallDecideContext {
        tool_name: "blocked".into(),
        tool_args: serde_json::Value::Null,
    });
    assert!(!denied.allow);
}

#[test]
fn convenience_on_post_tool_call() {
    use std::sync::{Arc, Mutex};

    let captured = Arc::new(Mutex::new(String::new()));
    let c = Arc::clone(&captured);

    let mut runner = Hooks::new();
    runner.on_post_tool_call("logger", move |ctx| {
        *c.lock().unwrap() = ctx.result.clone();
    });
    runner.run_post_tool_call(&PostToolCallContext {
        tool_name: "t".into(),
        tool_args: serde_json::Value::Null,
        result: "done".into(),
        metadata: serde_json::Value::Null,
    });
    assert_eq!(*captured.lock().unwrap(), "done");
}

#[test]
fn convenience_on_tool_error() {
    use std::sync::{Arc, Mutex};

    let captured = Arc::new(Mutex::new(String::new()));
    let c = Arc::clone(&captured);

    let mut runner = Hooks::new();
    runner.on_tool_error("err_log", move |ctx| {
        *c.lock().unwrap() = ctx.error.clone();
    });
    runner.run_on_tool_error(&OnToolErrorContext {
        tool_name: "t".into(),
        tool_args: serde_json::Value::Null,
        error: "boom".into(),
        metadata: serde_json::Value::Null,
    });
    assert_eq!(*captured.lock().unwrap(), "boom");
}

#[test]
fn convenience_on_compaction() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    let fired = Arc::new(AtomicBool::new(false));
    let f = Arc::clone(&fired);

    let mut runner = Hooks::new();
    runner.on_compaction("compact_log", move |_ctx| {
        f.store(true, Ordering::SeqCst);
    });
    runner.run_on_compaction(&OnCompactionContext {});
    assert!(fired.load(Ordering::SeqCst));
}

#[test]
fn convenience_on_interaction() {
    let mut runner = Hooks::new();
    runner.on_interaction("filter", |ctx| {
        if ctx.message.contains("spam") {
            HookResult::deny("spam detected")
        } else {
            HookResult::allow()
        }
    });

    let allowed = runner.run_on_interaction(&OnInteractionContext {
        message: "hello".into(),
    });
    assert!(allowed.allow);

    let denied = runner.run_on_interaction(&OnInteractionContext {
        message: "this is spam".into(),
    });
    assert!(!denied.allow);
}

#[test]
fn convenience_on_session_start() {
    use std::sync::{Arc, Mutex};

    let captured_id = Arc::new(Mutex::new(String::new()));
    let c = Arc::clone(&captured_id);

    let mut runner = Hooks::new();
    runner.on_session_start("log_start", move |ctx| {
        *c.lock().unwrap() = ctx.session.session_id.clone();
    });
    runner.run_on_session_start(&OnSessionStartContext {
        session: SessionContext {
            session_id: "sess-42".into(),
            agent_id: 7,
            started_at: SystemTime::now(),
        },
    });
    assert_eq!(*captured_id.lock().unwrap(), "sess-42");
}

#[test]
fn convenience_on_session_end() {
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };

    let captured_agent = Arc::new(AtomicU64::new(0));
    let c = Arc::clone(&captured_agent);

    let mut runner = Hooks::new();
    runner.on_session_end("log_end", move |ctx| {
        c.store(ctx.session.agent_id, Ordering::SeqCst);
    });
    runner.run_on_session_end(&OnSessionEndContext {
        session: SessionContext {
            session_id: "sess-99".into(),
            agent_id: 42,
            started_at: SystemTime::now(),
        },
    });
    assert_eq!(captured_agent.load(Ordering::SeqCst), 42);
}

// ── TransformToolInput tests ────────────────────────────────────────

#[test]
fn transform_tool_input_modifies_args() {
    let mut runner = Hooks::new();
    runner.on_transform_tool_input("inject_flag", |ctx| {
        let mut args = ctx.tool_args.clone();
        if let Some(obj) = args.as_object_mut() {
            obj.insert("injected".to_owned(), serde_json::Value::Bool(true));
        }
        Some(args)
    });

    let result = runner.run_transform_tool_input(&PreToolCallDecideContext {
        tool_name: "some_tool".into(),
        tool_args: serde_json::json!({"key": "value"}),
    });
    assert_eq!(result["key"], "value");
    assert_eq!(result["injected"], true);
}

#[test]
fn transform_tool_input_none_keeps_original() {
    let mut runner = Hooks::new();
    runner.on_transform_tool_input("noop", |_ctx| None);

    let original = serde_json::json!({"key": "value"});
    let result = runner.run_transform_tool_input(&PreToolCallDecideContext {
        tool_name: "t".into(),
        tool_args: original.clone(),
    });
    assert_eq!(result, original);
}

#[test]
fn transform_tool_input_chains_multiple() {
    let mut runner = Hooks::new();

    // First transform: add field_a
    runner.on_transform_tool_input("add_a", |ctx| {
        let mut args = ctx.tool_args.clone();
        if let Some(obj) = args.as_object_mut() {
            obj.insert("a".to_owned(), serde_json::json!(1));
        }
        Some(args)
    });

    // Second transform: add field_b
    runner.on_transform_tool_input("add_b", |ctx| {
        let mut args = ctx.tool_args.clone();
        if let Some(obj) = args.as_object_mut() {
            obj.insert("b".to_owned(), serde_json::json!(2));
        }
        Some(args)
    });

    let result = runner.run_transform_tool_input(&PreToolCallDecideContext {
        tool_name: "t".into(),
        tool_args: serde_json::json!({}),
    });
    assert_eq!(result["a"], 1);
    assert_eq!(result["b"], 2);
}

#[test]
fn transform_tool_input_panic_recovery() {
    let mut runner = Hooks::new();
    runner.on_transform_tool_input("panicker", |_ctx| {
        panic!("intentional test panic in transform hook");
    });

    let original = serde_json::json!({"safe": true});
    let result = runner.run_transform_tool_input(&PreToolCallDecideContext {
        tool_name: "t".into(),
        tool_args: original.clone(),
    });
    // After panic, original args should be preserved.
    assert_eq!(result, original);
}

#[test]
fn transform_callback_debug_format() {
    // NOLINT: |_| closure arg intentionally unused — mock callback for Debug format test
    let cb = HookCallback::TransformToolInput(Box::new(|_| None));
    let dbg = format!("{cb:?}");
    assert_eq!(dbg, "HookCallback::transform_tool_input");
}

// ── Builder chaining test ───────────────────────────────────────────

#[test]
fn convenience_builders_chain() {
    use std::sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    };

    let counter = Arc::new(AtomicU32::new(0));
    let c1 = Arc::clone(&counter);
    let c2 = Arc::clone(&counter);

    let mut runner = Hooks::new();
    runner
        .on_pre_turn("a", move |_ctx| {
            c1.fetch_add(1, Ordering::SeqCst);
        })
        .on_pre_turn("b", move |_ctx| {
            c2.fetch_add(10, Ordering::SeqCst);
        });

    runner.run_pre_turn(&PreTurnContext {
        prompt: "test".into(),
        turn_number: 1,
    });
    assert_eq!(counter.load(Ordering::SeqCst), 11);
}

#[test]
fn hooks_fluent_chaining() {
    use std::sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    };

    let counter = Arc::new(AtomicU32::new(0));
    let c1 = Arc::clone(&counter);
    let c2 = Arc::clone(&counter);

    let hooks = Hooks::new()
        .with_pre_turn("a", move |_ctx| {
            c1.fetch_add(1, Ordering::SeqCst);
        })
        .with_post_turn("b", move |_ctx| {
            c2.fetch_add(10, Ordering::SeqCst);
        });

    hooks.run_pre_turn(&PreTurnContext {
        prompt: "hi".into(),
        turn_number: 1,
    });
    hooks.run_post_turn(&PostTurnContext {
        response_text: "ok".into(),
        turn_number: 1,
    });
    assert_eq!(counter.load(Ordering::SeqCst), 11);
}

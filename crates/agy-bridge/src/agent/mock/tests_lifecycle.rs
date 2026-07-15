//! Agent lifecycle, initializing-hook cleanup, `chat_text()`, and subagent
//! lifecycle tests for the mock runtime.

use super::*;

// ── Agent lifecycle tests ────────────────────────────────────────

#[tokio::test]
async fn create_chat_shutdown_lifecycle() {
    let rt = Arc::new(ToolAwareMockRuntime::new());
    let agent = AgentHandle::new(rt, test_config(), None, None, None)
        .await
        .expect("create should succeed");

    assert!(agent.is_started());
    assert!(agent.id() > 0);

    {
        let mut response = agent.chat("Hello").await.expect("chat should succeed");
        if let Some(mut rx) = response.take_tool_call_stream() {
            let call = rx.recv().await.expect("should get tool call");
            assert_eq!(call.name, "add_numbers");
        }
    }

    agent.shutdown().await.expect("shutdown should succeed");
    assert!(!agent.is_started());
}

#[tokio::test]
async fn create_with_invalid_config_returns_error() {
    let rt = Arc::new(ToolAwareMockRuntime::with_create_failure());
    let result = AgentHandle::new(rt, test_config(), None, None, None).await;

    match result {
        Err(Error::BackendError { message }) => {
            assert!(message.contains("invalid config"));
        }
        Err(other) => panic!("Expected BackendError, got: {other:?}"),
        Ok(_) => panic!("Expected error, got Ok"),
    }
}

const TEST_MAX_QUOTA_RETRIES: u32 = 1000;

#[tokio::test]
async fn chat_with_quota_backoff_retries() {
    let rt = Arc::new(ToolAwareMockRuntime::new());
    rt.fail_quota.store(true, Ordering::SeqCst);

    let config = AgentConfig::builder()
        .max_quota_retries(TEST_MAX_QUOTA_RETRIES)
        .build();
    let agent = AgentHandle::new(Arc::clone(&rt), config, None, None, None)
        .await
        .expect("create should succeed");

    {
        let mut response = agent
            .chat("Hello")
            .await
            .expect("should succeed after retry");
        if let Some(mut rx) = response.take_tool_call_stream() {
            let _call = rx.recv().await;
        }
    }

    agent.shutdown().await.expect("shutdown should succeed");
}

#[tokio::test]
async fn conversation_id_tracking() {
    let rt = Arc::new(ToolAwareMockRuntime::new());
    let agent = AgentHandle::new(rt, test_config(), None, None, None)
        .await
        .expect("create should succeed");

    assert!(agent.conversation_id().is_none());

    agent.set_conversation_id("conv_abc123".to_owned());
    assert_eq!(agent.conversation_id().as_deref(), Some("conv_abc123"));

    agent.shutdown().await.expect("shutdown should succeed");
}

#[tokio::test]
async fn ffi_session_start_does_not_inject_session_id_as_conversation_id() {
    let rt = Arc::new(ToolAwareMockRuntime::new());
    let agent = AgentHandle::new(rt, test_config(), None, None, None)
        .await
        .expect("create should succeed");

    assert!(agent.conversation_id().is_none());

    // Simulate the dispatch_rust_hook callback for on_session_start.
    // Previously this would inject session_id as conversation_id, but
    // session_id is the save-directory basename (e.g. "fixed_run_3"),
    // NOT a real conversation handle.  The fix ensures conversation_id
    // stays None unless explicitly set via set_conversation_id().
    let ctx = crate::hooks::OnSessionStartContext {
        session: crate::hooks::SessionContext {
            session_id: "dynamically-generated-session-123".to_owned(),
            agent_id: agent.id(),
            started_at: std::time::SystemTime::now(),
        },
    };
    let ctx_json = serde_json::to_string(&ctx).unwrap();

    // Simulate hook callback execution using the internal dispatch function
    let hook_runner = {
        let map = crate::runtime::bridge_state().read().unwrap();
        let entry = map.get(&agent.id()).unwrap();
        Arc::clone(entry.hook_runner.as_ref().unwrap())
    };

    crate::runtime::ffi_dispatch::dispatch_hook_by_name(
        agent.id(),
        &hook_runner,
        "on_session_start",
        &ctx_json,
    )
    .unwrap();

    // conversation_id must remain None — session_id is NOT a conversation handle
    assert!(
        agent.conversation_id().is_none(),
        "on_session_start must NOT inject session_id as conversation_id"
    );

    agent.shutdown().await.expect("shutdown should succeed");
}

#[tokio::test]
async fn double_shutdown_is_idempotent() {
    let rt = Arc::new(ToolAwareMockRuntime::new());
    let agent = AgentHandle::new(rt, test_config(), None, None, None)
        .await
        .expect("create should succeed");

    agent
        .shutdown()
        .await
        .expect("first shutdown should succeed");
    assert!(!agent.is_started());

    agent
        .shutdown()
        .await
        .expect("second shutdown should succeed");
}

#[tokio::test]
async fn drop_without_shutdown_calls_try_shutdown() {
    let rt = Arc::new(ToolAwareMockRuntime::new());
    let agent = AgentHandle::new(Arc::clone(&rt), test_config(), None, None, None)
        .await
        .expect("create should succeed");

    assert!(!rt.try_shutdown_called.load(Ordering::SeqCst));
    drop(agent);
    assert!(
        rt.try_shutdown_called.load(Ordering::SeqCst),
        "Drop should call try_shutdown_agent"
    );
}

#[tokio::test]
async fn drop_after_shutdown_does_not_call_try_shutdown() {
    let rt = Arc::new(ToolAwareMockRuntime::new());
    let agent = AgentHandle::new(Arc::clone(&rt), test_config(), None, None, None)
        .await
        .expect("create should succeed");

    agent.shutdown().await.expect("shutdown");
    assert!(!rt.try_shutdown_called.load(Ordering::SeqCst));

    drop(agent);
    assert!(
        !rt.try_shutdown_called.load(Ordering::SeqCst),
        "Drop after explicit shutdown should NOT call try_shutdown_agent"
    );
}

// ── InitializingHookGuard cleanup tests ────────────────────────────
//
// These guard the lock-free, per-agent-keyed creation path: the entry a
// creating agent installs in `initializing_hook_runners()` must never
// outlive `AgentHandle::new`, on ANY exit path. All tests key on a
// globally-unique `next_agent_id()` so they are deterministic even when
// run in parallel with other tests that share the global registry.

/// Dropping the guard removes exactly its own entry and nothing else.
#[tokio::test]
async fn initializing_hook_guard_removes_entry_on_drop() {
    let id = crate::runtime::next_agent_id();
    let other = crate::runtime::next_agent_id();
    let hooks = Arc::new(crate::hooks::Hooks::new());

    {
        let mut map = crate::runtime::initializing_hook_runners()
            .write()
            .expect("registry writable");
        map.insert(id, Arc::clone(&hooks));
        map.insert(other, Arc::clone(&hooks));
    }

    {
        let _guard = super::super::InitializingHookGuard(id);
        assert!(
            crate::runtime::initializing_hook_runners()
                .read()
                .expect("registry readable")
                .contains_key(&id),
            "entry must exist while guard is alive"
        );
    } // guard drops here

    let map = crate::runtime::initializing_hook_runners()
        .read()
        .expect("registry readable");
    assert!(
        !map.contains_key(&id),
        "guard drop must remove its own entry"
    );
    assert!(
        map.contains_key(&other),
        "guard drop must NOT remove unrelated entries"
    );
    drop(map);

    // Cleanup the unrelated entry we inserted.
    crate::runtime::initializing_hook_runners()
        .write()
        .expect("registry writable")
        .remove(&other);
}

/// A successful `AgentHandle::new` must leave no lingering entry for its ID.
#[tokio::test]
async fn successful_create_leaves_no_initializing_entry() {
    let rt = Arc::new(ToolAwareMockRuntime::new());
    let agent = AgentHandle::new(rt, test_config(), None, None, None)
        .await
        .expect("create should succeed");

    assert!(
        !crate::runtime::initializing_hook_runners()
            .read()
            .expect("registry readable")
            .contains_key(&agent.id()),
        "successful create must not leave an initializing hook entry"
    );

    agent.shutdown().await.expect("shutdown should succeed");
}

/// A FAILED `AgentHandle::new` must still clean up its initializing entry —
/// the RAII-on-error path. The mock records the exact `agent_id` it was
/// handed, so we can assert cleanup for that precise key.
#[tokio::test]
async fn failed_create_cleans_up_initializing_entry() {
    let rt = Arc::new(ToolAwareMockRuntime::with_create_failure());
    let result = AgentHandle::new(Arc::clone(&rt), test_config(), None, None, None).await;
    assert!(result.is_err(), "create was configured to fail");

    let failed_id = rt
        .last_create_id
        .lock()
        .expect("mock id mutex")
        .expect("create_agent must have recorded an id before failing");

    assert!(
        !crate::runtime::initializing_hook_runners()
            .read()
            .expect("registry readable")
            .contains_key(&failed_id),
        "failed create must not leak an initializing hook entry for id {failed_id}"
    );
}

// ── chat_text() tests ──────────────────────────────────────────────

#[tokio::test]
async fn chat_text_returns_text() {
    let rt = Arc::new(ToolAwareMockRuntime::new());
    let agent = AgentHandle::new(Arc::clone(&rt), test_config(), None, None, None)
        .await
        .expect("create agent");

    let text = agent.chat_text("Hello").await.expect("chat_text");
    assert!(!text.is_empty());

    agent.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn cancel_completes_successfully() {
    let rt = Arc::new(ToolAwareMockRuntime::new());
    let agent = AgentHandle::new(rt, test_config(), None, None, None)
        .await
        .expect("create agent");
    agent.cancel().await.expect("cancel should succeed");
}

#[tokio::test]
async fn wait_for_idle_completes_successfully() {
    let rt = Arc::new(ToolAwareMockRuntime::new());
    let agent = AgentHandle::new(rt, test_config(), None, None, None)
        .await
        .expect("create agent");
    agent
        .wait_for_idle()
        .await
        .expect("wait_for_idle should succeed");
}

#[tokio::test]
async fn send_completes_successfully() {
    let rt = Arc::new(ToolAwareMockRuntime::new());
    let agent = AgentHandle::new(rt, test_config(), None, None, None)
        .await
        .expect("create agent");
    agent
        .send("fire-and-forget message")
        .await
        .expect("send should succeed");
}

#[tokio::test]
async fn signal_idle_completes_successfully() {
    let rt = Arc::new(ToolAwareMockRuntime::new());
    let agent = AgentHandle::new(rt, test_config(), None, None, None)
        .await
        .expect("create agent");
    agent
        .signal_idle()
        .await
        .expect("signal_idle should succeed");
}

#[tokio::test]
async fn wait_for_wakeup_returns_false_on_mock_timeout() {
    let rt = Arc::new(ToolAwareMockRuntime::new());
    let agent = AgentHandle::new(rt, test_config(), None, None, None)
        .await
        .expect("create agent");
    let woken = agent
        .wait_for_wakeup(Duration::from_secs(1))
        .await
        .expect("wait_for_wakeup should succeed");
    assert!(!woken, "mock should return false (timeout)");
}

// ── Audit 4: Subagent lifecycle tests ──────────────────────────────

#[tokio::test]
async fn spawn_subagent_creates_child() {
    let rt = Arc::new(ToolAwareMockRuntime::new());
    let parent = AgentHandle::new(Arc::clone(&rt), test_config(), None, None, None)
        .await
        .expect("create parent");
    let parent_id = parent.id();

    let child = parent
        .spawn_subagent(test_config(), None)
        .await
        .expect("spawn subagent");
    let child_id = child.id();

    // Child should have a distinct ID.
    assert_ne!(parent_id, child_id);
    assert!(child.is_started());
}

#[tokio::test]
async fn spawn_subagent_with_registry_populates_tools() {
    #[derive(serde::Deserialize, schemars::JsonSchema)]
    struct Params {
        x: i32,
    }
    struct TestTool;
    impl crate::tools::RustTool for TestTool {
        type Params = Params;
        const NAME: &'static str = "test_tool";
        const DESCRIPTION: &'static str = "A test tool";
        async fn call(
            &self,
            params: Params,
            _ctx: &crate::tools::ToolContext,
        ) -> Result<crate::tools::ToolOutput, crate::tools::ToolError> {
            assert!(params.x > 0, "expected positive x, got {}", params.x);
            Ok("ok".into())
        }
    }

    let rt = Arc::new(ToolAwareMockRuntime::new());
    let parent = AgentHandle::new(Arc::clone(&rt), test_config(), None, None, None)
        .await
        .expect("create parent");

    let mut registry = crate::tools::ToolRegistry::new();
    registry.register(TestTool);

    let child = parent
        .spawn_subagent(test_config(), registry)
        .await
        .expect("spawn subagent with registry");

    // The child's config should have the tool definition from the registry.
    assert_eq!(child.config().tools.len(), 1);
    assert_eq!(child.config().tools[0].name, "test_tool");
}

#[tokio::test]
async fn subagent_shutdown_lifecycle() {
    let rt = Arc::new(ToolAwareMockRuntime::new());
    let parent = AgentHandle::new(Arc::clone(&rt), test_config(), None, None, None)
        .await
        .expect("create parent");

    let child = parent
        .spawn_subagent(test_config(), None)
        .await
        .expect("spawn subagent");

    // Shut down the child.
    child.shutdown().await.expect("shutdown child");
    assert!(!child.is_started());
}

//! Conversation-layer, structured-output/usage, multi-agent isolation, and
//! `available_tools()` tests for the mock runtime.

use super::*;

// ── Conversation layer tests ──────────────────────────────────────

#[tokio::test]
async fn history_returns_messages() {
    let rt = Arc::new(ToolAwareMockRuntime::new());
    let agent = AgentHandle::new(rt, test_config(), None, None, None)
        .await
        .expect("create agent");

    let history = agent.history().await.expect("history should succeed");
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].role, MessageRole::User);
    assert_eq!(history[0].content, "Hello");
    assert_eq!(history[1].role, MessageRole::Model);
    assert_eq!(history[1].content, "Hi there!");
}

#[tokio::test]
async fn turn_count_returns_zero_initially() {
    let rt = Arc::new(ToolAwareMockRuntime::new());
    let agent = AgentHandle::new(rt, test_config(), None, None, None)
        .await
        .expect("create agent");

    let count = agent.turn_count().await.expect("turn_count should succeed");
    assert_eq!(count, 0);
}

#[tokio::test]
async fn turn_count_increments_after_chat() {
    let rt = Arc::new(ToolAwareMockRuntime::new());
    let agent = AgentHandle::new(Arc::clone(&rt), test_config(), None, None, None)
        .await
        .expect("create agent");

    let _response = agent.chat("Hello").await.expect("chat should succeed");
    let count = agent.turn_count().await.expect("turn_count");
    assert_eq!(count, 1);
}

#[tokio::test]
async fn clear_history_resets_turn_count() {
    let rt = Arc::new(ToolAwareMockRuntime::new());
    let agent = AgentHandle::new(Arc::clone(&rt), test_config(), None, None, None)
        .await
        .expect("create agent");

    let _response = agent.chat("Hello").await.expect("chat should succeed");
    assert_eq!(agent.turn_count().await.unwrap(), 1);

    agent.clear_history().await.expect("clear_history");
    assert_eq!(agent.turn_count().await.unwrap(), 0);
}

#[tokio::test]
async fn remove_last_turn_decrements_turn_count() {
    let rt = Arc::new(ToolAwareMockRuntime::new());
    let agent = AgentHandle::new(Arc::clone(&rt), test_config(), None, None, None)
        .await
        .expect("create agent");

    let _r1 = agent.chat("Turn 1").await.expect("chat 1");
    let _r2 = agent.chat("Turn 2").await.expect("chat 2");
    assert_eq!(agent.turn_count().await.unwrap(), 2);

    agent
        .remove_last_turn()
        .await
        .expect("remove_last_turn should succeed");
    assert_eq!(agent.turn_count().await.unwrap(), 1);
}

#[tokio::test]
async fn remove_last_turn_saturates_at_zero() {
    let rt = Arc::new(ToolAwareMockRuntime::new());
    let agent = AgentHandle::new(Arc::clone(&rt), test_config(), None, None, None)
        .await
        .expect("create agent");

    assert_eq!(agent.turn_count().await.unwrap(), 0);

    // Should not panic or underflow when no turns exist.
    agent
        .remove_last_turn()
        .await
        .expect("remove_last_turn on empty history should succeed");
    assert_eq!(agent.turn_count().await.unwrap(), 0);
}

#[tokio::test]
async fn total_usage_returns_metadata() {
    let rt = Arc::new(ToolAwareMockRuntime::new());
    let agent = AgentHandle::new(rt, test_config(), None, None, None)
        .await
        .expect("create agent");

    let usage = agent.total_usage().await.expect("total_usage");
    assert_eq!(usage.prompt_token_count, Some(500));
    assert_eq!(usage.total_token_count, Some(800));
}

#[tokio::test]
async fn last_turn_usage_returns_metadata() {
    let rt = Arc::new(ToolAwareMockRuntime::new());
    let agent = AgentHandle::new(rt, test_config(), None, None, None)
        .await
        .expect("create agent");

    let usage = agent.last_turn_usage().await.expect("last_turn_usage");
    assert_eq!(usage.prompt_token_count, Some(100));
    assert_eq!(usage.total_token_count, Some(170));
}

#[tokio::test]
async fn get_last_structured_output_none_initially() {
    let rt = Arc::new(ToolAwareMockRuntime::new());
    let agent = AgentHandle::new(rt, test_config(), None, None, None)
        .await
        .expect("create agent");

    assert!(agent.get_last_structured_output().is_none());
}

#[tokio::test]
async fn get_last_structured_output_after_chat() {
    let rt = Arc::new(ToolAwareMockRuntime::new());
    let agent = AgentHandle::new(Arc::clone(&rt), test_config(), None, None, None)
        .await
        .expect("create agent");

    let mut response = agent.chat("Hello").await.expect("chat should succeed");
    // Set structured output via the shared state (the canonical path)
    {
        let mut state = response.shared_state.lock().unwrap();
        state.structured_output = Some(serde_json::json!({"answer": 42}));
    }
    // Finalize to pull shared state into the handle
    response.finalize();
    drop(response);

    let so = agent.get_last_structured_output();
    assert_eq!(so, Some(serde_json::json!({"answer": 42})));
}

#[derive(serde::Deserialize, Debug, PartialEq)]
struct AnswerOutput {
    answer: i64,
}

#[tokio::test]
async fn get_last_structured_output_as_typed_success() {
    let rt = Arc::new(ToolAwareMockRuntime::new());
    let agent = AgentHandle::new(Arc::clone(&rt), test_config(), None, None, None)
        .await
        .expect("create agent");

    let mut response = agent.chat("Hello").await.expect("chat should succeed");
    {
        let mut state = response.shared_state.lock().unwrap();
        state.structured_output = Some(serde_json::json!({"answer": 42}));
    }
    response.finalize();
    drop(response);

    let typed = agent.get_last_structured_output_as::<AnswerOutput>();
    assert_eq!(
        typed.expect("some").expect("ok"),
        AnswerOutput { answer: 42 }
    );
}

#[tokio::test]
async fn get_last_structured_output_as_typed_failure() {
    let rt = Arc::new(ToolAwareMockRuntime::new());
    let agent = AgentHandle::new(Arc::clone(&rt), test_config(), None, None, None)
        .await
        .expect("create agent");

    let mut response = agent.chat("Hello").await.expect("chat should succeed");
    {
        let mut state = response.shared_state.lock().unwrap();
        state.structured_output = Some(serde_json::json!({"wrong_field": "hello"}));
    }
    response.finalize();
    drop(response);

    let typed = agent.get_last_structured_output_as::<AnswerOutput>();
    assert!(typed.expect("some").is_err());
}

#[tokio::test]
async fn get_last_usage_none_initially() {
    let rt = Arc::new(ToolAwareMockRuntime::new());
    let agent = AgentHandle::new(rt, test_config(), None, None, None)
        .await
        .expect("create agent");

    assert!(agent.get_last_usage().is_none());
}

#[tokio::test]
async fn get_last_usage_after_chat() {
    let rt = Arc::new(ToolAwareMockRuntime::new());
    let agent = AgentHandle::new(Arc::clone(&rt), test_config(), None, None, None)
        .await
        .expect("create agent");

    let response = agent.chat("Hello").await.expect("chat should succeed");

    // Simulate usage being populated in shared state
    {
        let mut state = response.shared_state.lock().unwrap();
        state.usage = Some(UsageMetadata {
            prompt_token_count: Some(50),
            cached_content_token_count: None,
            candidates_token_count: Some(25),
            thoughts_token_count: None,
            total_token_count: Some(75),
        });
    }

    let usage = agent.get_last_usage().expect("should have usage");
    assert_eq!(usage.prompt_token_count, Some(50));
    assert_eq!(usage.total_token_count, Some(75));
}

#[tokio::test]
async fn conversation_message_serde_roundtrip() {
    let msg = ConversationMessage {
        role: MessageRole::User,
        content: "Hello, world!".to_string(),
    };
    let json = serde_json::to_string(&msg).expect("serialize");
    let parsed: ConversationMessage = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed, msg);
}

// ── New conversation methods ──────────────────────────────────────

#[tokio::test]
async fn last_response_returns_text() {
    let rt = Arc::new(ToolAwareMockRuntime::new());
    let agent = AgentHandle::new(rt, test_config(), None, None, None)
        .await
        .expect("create agent");

    let resp = agent.last_response().await.expect("last_response");
    assert_eq!(resp.as_deref(), Some("Hi there!"));
}

#[tokio::test]
async fn compaction_indices_returns_indices() {
    let rt = Arc::new(ToolAwareMockRuntime::new());
    let agent = AgentHandle::new(rt, test_config(), None, None, None)
        .await
        .expect("create agent");

    let indices = agent
        .compaction_indices()
        .await
        .expect("compaction_indices");
    assert_eq!(indices, vec![3, 7]);
}

#[tokio::test]
async fn delete_marks_agent_as_shutdown() {
    let rt = Arc::new(ToolAwareMockRuntime::new());
    let agent = AgentHandle::new(rt, test_config(), None, None, None)
        .await
        .expect("create agent");

    assert!(agent.is_started());
    agent.delete().await.expect("delete");
    assert!(!agent.is_started());
}

#[tokio::test]
async fn disconnect_marks_agent_as_shutdown() {
    let rt = Arc::new(ToolAwareMockRuntime::new());
    let agent = AgentHandle::new(rt, test_config(), None, None, None)
        .await
        .expect("create agent");

    assert!(agent.is_started());
    agent.disconnect().await.expect("disconnect");
    assert!(!agent.is_started());
}

#[tokio::test]
async fn is_idle_returns_true_on_mock() {
    let rt = Arc::new(ToolAwareMockRuntime::new());
    let agent = AgentHandle::new(rt, test_config(), None, None, None)
        .await
        .expect("create agent");

    let idle = agent.is_idle().await.expect("is_idle");
    assert!(idle, "mock should report idle");
}

// ── Multi-agent isolation tests ─────────────────────────────────

#[tokio::test]
async fn multiple_agents_same_runtime_have_distinct_ids() {
    let rt = Arc::new(ToolAwareMockRuntime::new());
    let a = AgentHandle::new(Arc::clone(&rt), test_config(), None, None, None)
        .await
        .expect("agent a");
    let b = AgentHandle::new(Arc::clone(&rt), test_config(), None, None, None)
        .await
        .expect("agent b");

    assert_ne!(a.id(), b.id(), "agents must have distinct IDs");
    a.shutdown().await.expect("shutdown a");
    b.shutdown().await.expect("shutdown b");
}

#[tokio::test]
async fn shutdown_one_agent_does_not_affect_another() {
    let rt = Arc::new(ToolAwareMockRuntime::new());
    let a = AgentHandle::new(Arc::clone(&rt), test_config(), None, None, None)
        .await
        .expect("agent a");
    let b = AgentHandle::new(Arc::clone(&rt), test_config(), None, None, None)
        .await
        .expect("agent b");

    let a_id = a.id();
    let b_id = b.id();

    // Both agents have bridge state entries.
    {
        let map = crate::runtime::bridge_state()
            .read()
            .expect("read bridge state");
        assert!(map.contains_key(&a_id), "agent a must have bridge state");
        assert!(map.contains_key(&b_id), "agent b must have bridge state");
    }

    // Shut down agent a.
    a.shutdown().await.expect("shutdown a");
    drop(a);

    // Agent b's bridge state must still be present.
    {
        let map = crate::runtime::bridge_state()
            .read()
            .expect("read bridge state");
        assert!(
            !map.contains_key(&a_id),
            "agent a bridge state should be cleaned up after shutdown+drop"
        );
        assert!(
            map.contains_key(&b_id),
            "agent b bridge state must survive agent a's shutdown"
        );
    }

    // Agent b still works.
    let text = b.chat("hello").await.expect("chat b");
    let content = text.text().await.expect("text b");
    assert!(!content.is_empty(), "agent b should return non-empty text");

    b.shutdown().await.expect("shutdown b");
}

#[tokio::test]
async fn agents_on_different_runtimes_are_isolated() {
    let rt1 = Arc::new(ToolAwareMockRuntime::new());
    let rt2 = Arc::new(ToolAwareMockRuntime::new());

    let a = AgentHandle::new(rt1, test_config(), None, None, None)
        .await
        .expect("agent a");
    let b = AgentHandle::new(rt2, test_config(), None, None, None)
        .await
        .expect("agent b");

    // Both should have bridge state.
    {
        let map = crate::runtime::bridge_state()
            .read()
            .expect("read bridge state");
        assert!(map.contains_key(&a.id()), "agent a bridge state");
        assert!(map.contains_key(&b.id()), "agent b bridge state");
    }

    // Chat on both concurrently.
    let (res_a, res_b) = tokio::join!(a.chat("hello"), b.chat("world"));
    let text_a = res_a.expect("chat a").text().await.expect("text a");
    let text_b = res_b.expect("chat b").text().await.expect("text b");
    assert!(!text_a.is_empty());
    assert!(!text_b.is_empty());

    a.shutdown().await.expect("shutdown a");
    b.shutdown().await.expect("shutdown b");
}

#[tokio::test]
async fn multiple_agents_with_different_configs() {
    let rt = Arc::new(ToolAwareMockRuntime::new());

    let config_a = AgentConfig::builder().model("model-alpha").build();
    let config_b = AgentConfig::builder().model("model-beta").build();

    let a = AgentHandle::new(Arc::clone(&rt), config_a, None, None, None)
        .await
        .expect("agent a");
    let b = AgentHandle::new(Arc::clone(&rt), config_b, None, None, None)
        .await
        .expect("agent b");

    assert_ne!(a.id(), b.id());

    // Each agent maintains its own config.
    assert_eq!(a.config().model, "model-alpha");
    assert_eq!(b.config().model, "model-beta");

    a.shutdown().await.expect("shutdown a");
    b.shutdown().await.expect("shutdown b");
}

#[tokio::test]
async fn dropping_one_agent_preserves_others_bridge_state() {
    let rt = Arc::new(ToolAwareMockRuntime::new());

    let a = AgentHandle::new(Arc::clone(&rt), test_config(), None, None, None)
        .await
        .expect("agent a");
    let b = AgentHandle::new(Arc::clone(&rt), test_config(), None, None, None)
        .await
        .expect("agent b");
    let c = AgentHandle::new(Arc::clone(&rt), test_config(), None, None, None)
        .await
        .expect("agent c");

    let b_id = b.id();
    let c_id = c.id();

    // Drop a without shutdown (best-effort path).
    drop(a);

    // b and c must still be fully functional.
    {
        let map = crate::runtime::bridge_state()
            .read()
            .expect("read bridge state");
        assert!(map.contains_key(&b_id), "agent b bridge state must survive");
        assert!(map.contains_key(&c_id), "agent c bridge state must survive");
    }

    let text = b.chat("test").await.expect("chat b");
    let content = text.text().await.expect("text b");
    assert!(!content.is_empty());

    b.shutdown().await.expect("shutdown b");
    c.shutdown().await.expect("shutdown c");
}

// ── available_tools() tests ───────────────────────────────────────

#[tokio::test]
async fn available_tools_populated_at_creation() {
    let rt = Arc::new(ToolAwareMockRuntime::new());
    let agent = AgentHandle::new(rt, test_config(), None, None, None)
        .await
        .expect("create agent");

    let tools = agent.available_tools();
    assert!(!tools.is_empty(), "available_tools should not be empty");

    let names = agent.available_tool_names();
    assert!(
        names.contains(&"add_numbers"),
        "should contain mock tool 'add_numbers', got: {names:?}"
    );
    assert!(
        names.contains(&"run_command"),
        "should contain mock tool 'run_command', got: {names:?}"
    );
    assert!(
        names.contains(&"view_file"),
        "should contain mock tool 'view_file', got: {names:?}"
    );

    // Verify source tagging.
    let custom = tools
        .iter()
        .find(|t| t.name == "add_numbers")
        .expect("add_numbers tool");
    assert_eq!(custom.source, crate::tools::ToolSource::Custom);
    assert!(
        !custom.description.is_empty(),
        "custom tool should have description"
    );

    let builtin = tools
        .iter()
        .find(|t| t.name == "run_command")
        .expect("run_command tool");
    assert_eq!(builtin.source, crate::tools::ToolSource::Builtin);
    assert!(
        !builtin.description.is_empty(),
        "builtin tool should have description"
    );

    agent.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn available_tools_survives_chat() {
    let rt = Arc::new(ToolAwareMockRuntime::new());
    let agent = AgentHandle::new(Arc::clone(&rt), test_config(), None, None, None)
        .await
        .expect("create agent");

    let _response = agent.chat("Hello").await.expect("chat");

    // available_tools must still be accessible after chat
    let tools = agent.available_tools();
    assert!(
        !tools.is_empty(),
        "available_tools should persist after chat"
    );

    agent.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn subagent_has_own_available_tools() {
    let rt = Arc::new(ToolAwareMockRuntime::new());
    let parent = AgentHandle::new(Arc::clone(&rt), test_config(), None, None, None)
        .await
        .expect("create parent");

    let child = parent
        .spawn_subagent(test_config(), None)
        .await
        .expect("spawn subagent");

    // Both parent and child should have available tools
    assert!(!parent.available_tools().is_empty());
    assert!(!child.available_tools().is_empty());

    child.shutdown().await.expect("shutdown child");
    parent.shutdown().await.expect("shutdown parent");
}

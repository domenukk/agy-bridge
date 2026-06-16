//! Mock runtime for agent unit testing.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use super::{AgentHandle, AgentId, Runtime};
use crate::{
    config::AgentConfig,
    content::Content,
    error::Error,
    streaming::{self, ChatResponseHandle},
    types::{ConversationMessage, MessageRole, UsageMetadata},
};

/// A mock runtime that simulates tool calls on the first `chat()` call
/// and returns text on subsequent calls, enabling tests of the `chat_text()`
/// agentic loop without a live Python runtime.
pub struct ToolAwareMockRuntime {
    next_id: AtomicU64,
    /// Counts how many times `chat()` has been called per agent.
    /// First call → tool call; subsequent calls → text response.
    chat_count: std::sync::Mutex<std::collections::HashMap<AgentId, u32>>,
    /// If true, `create_agent` will fail.
    fail_create: AtomicBool,
    /// If true, first `chat` will return `QuotaExceeded` (then resets).
    fail_quota: AtomicBool,
    /// Tracks whether `try_shutdown_agent` was called (from Drop).
    pub(crate) try_shutdown_called: AtomicBool,
    /// Per-runtime quota registry.
    quota_registry: crate::quota::QuotaRegistry,
}

impl ToolAwareMockRuntime {
    pub(crate) fn new() -> Self {
        Self {
            next_id: AtomicU64::new(1),
            chat_count: std::sync::Mutex::new(std::collections::HashMap::new()),
            fail_create: AtomicBool::new(false),
            fail_quota: AtomicBool::new(false),
            try_shutdown_called: AtomicBool::new(false),
            quota_registry: crate::quota::QuotaRegistry::new(),
        }
    }

    pub(crate) fn with_create_failure() -> Self {
        let rt = Self::new();
        rt.fail_create.store(true, Ordering::SeqCst);
        rt
    }
}

impl Runtime for ToolAwareMockRuntime {
    async fn create_agent(&self, _config: AgentConfig) -> Result<AgentId, Error> {
        if self.fail_create.load(Ordering::SeqCst) {
            return Err(Error::BackendError {
                message: "invalid config: missing system instructions".to_owned(),
            });
        }
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(id)
    }

    /// # Panics
    ///
    /// Panics if the internal `chat_count` mutex is poisoned. This is
    /// acceptable because `ToolAwareMockRuntime` is only compiled under
    /// `#[cfg(test)]`.
    async fn chat(
        &self,
        agent_id: AgentId,
        _content: &Content,
    ) -> Result<ChatResponseHandle, Error> {
        if self.fail_quota.load(Ordering::SeqCst) {
            self.fail_quota.store(false, Ordering::SeqCst);
            return Err(Error::QuotaExceeded {
                retry_after: Duration::from_millis(10),
            });
        }

        let call_num = {
            let mut counts = self.chat_count.lock().unwrap();
            let entry = counts.entry(agent_id).or_insert(0);
            *entry += 1;
            *entry
        };

        let (writer, handle) = streaming::channel();
        if call_num == 1 {
            tokio::spawn(async move {
                if let Err(e) = writer
                    .tool_call_tx
                    .send(crate::streaming::ToolCallEvent {
                        name: "add_numbers".to_owned(),
                        args: serde_json::json!({"x": 2, "y": 3}),
                        id: Some("call_1".to_owned()),
                        canonical_path: None,
                    })
                    .await
                {
                    tracing::error!("Mock tool_call send failed: {e}");
                }
                if let Err(e) = writer.text_tx.send("Mock text response".to_owned()).await {
                    tracing::error!("Mock text send failed: {e}");
                }
            });
        } else {
            tokio::spawn(async move {
                if let Err(e) = writer
                    .text_tx
                    .send("Tool result received, final answer: 5".to_owned())
                    .await
                {
                    tracing::error!("Mock text send failed: {e}");
                }
            });
        }
        Ok(handle)
    }

    async fn shutdown_agent(&self, _agent_id: AgentId) -> Result<(), Error> {
        Ok(())
    }

    async fn cancel(&self, _agent_id: AgentId) -> Result<(), Error> {
        Ok(())
    }

    async fn wait_for_idle(&self, _agent_id: AgentId) -> Result<(), Error> {
        Ok(())
    }

    async fn send(&self, _agent_id: AgentId, _content: &Content) -> Result<(), Error> {
        Ok(())
    }

    async fn signal_idle(&self, _agent_id: AgentId) -> Result<(), Error> {
        Ok(())
    }

    async fn wait_for_wakeup(&self, _agent_id: AgentId, _timeout: Duration) -> Result<bool, Error> {
        // Mock always times out (returns false).
        Ok(false)
    }

    async fn wait_for_quota(&self) {}

    async fn record_quota_hit(&self, _retry_after: Duration) {}

    fn quota_registry(&self) -> &crate::quota::QuotaRegistry {
        &self.quota_registry
    }

    async fn history(&self, _agent_id: AgentId) -> Result<Vec<ConversationMessage>, Error> {
        Ok(vec![
            ConversationMessage {
                role: MessageRole::User,
                content: "Hello".to_string(),
            },
            ConversationMessage {
                role: MessageRole::Model,
                content: "Hi there!".to_string(),
            },
        ])
    }

    async fn turn_count(&self, agent_id: AgentId) -> Result<u32, Error> {
        let counts = self.chat_count.lock().unwrap();
        Ok(*counts.get(&agent_id).unwrap_or(&0))
    }

    async fn total_usage(&self, _agent_id: AgentId) -> Result<UsageMetadata, Error> {
        Ok(UsageMetadata {
            prompt_token_count: Some(500),
            cached_content_token_count: None,
            candidates_token_count: Some(200),
            thoughts_token_count: Some(100),
            total_token_count: Some(800),
        })
    }

    async fn last_turn_usage(&self, _agent_id: AgentId) -> Result<UsageMetadata, Error> {
        Ok(UsageMetadata {
            prompt_token_count: Some(100),
            cached_content_token_count: None,
            candidates_token_count: Some(50),
            thoughts_token_count: Some(20),
            total_token_count: Some(170),
        })
    }

    async fn clear_history(&self, agent_id: AgentId) -> Result<(), Error> {
        {
            let mut counts = self.chat_count.lock().unwrap();
            counts.insert(agent_id, 0);
        }
        Ok(())
    }

    async fn last_response(&self, _agent_id: AgentId) -> Result<Option<String>, Error> {
        Ok(Some("Hi there!".to_string()))
    }

    async fn compaction_indices(&self, _agent_id: AgentId) -> Result<Vec<u32>, Error> {
        Ok(vec![3, 7])
    }

    async fn delete(&self, _agent_id: AgentId) -> Result<(), Error> {
        Ok(())
    }

    async fn disconnect(&self, _agent_id: AgentId) -> Result<(), Error> {
        Ok(())
    }

    async fn is_idle(&self, _agent_id: AgentId) -> Result<bool, Error> {
        Ok(true)
    }

    fn try_shutdown_agent(&self, _agent_id: AgentId) {
        self.try_shutdown_called.store(true, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> AgentConfig {
        AgentConfig::default()
    }

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
}

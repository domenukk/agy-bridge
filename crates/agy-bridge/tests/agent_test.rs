use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU32, Ordering},
};

use agy_bridge::{error::Error, prelude::*};

/// Mock runtime with tracked state for testing `AgentHandle` without a live
/// Python interpreter.
struct MockRuntime {
    /// Tracks conversation history per agent.
    history: Mutex<Vec<agy_bridge::types::ConversationMessage>>,
    /// Tracks the current turn count.
    turn_count: AtomicU32,
    /// Per-runtime quota registry.
    quota_registry: agy_bridge::quota::QuotaRegistry,
}

impl MockRuntime {
    fn new() -> Self {
        Self {
            history: Mutex::new(Vec::new()),
            turn_count: AtomicU32::new(0),
            quota_registry: agy_bridge::quota::QuotaRegistry::new(),
        }
    }
}

impl agy_bridge::agent::Runtime for MockRuntime {
    async fn create_agent(
        &self,
        _config: AgentConfig,
    ) -> Result<agy_bridge::agent::AgentId, Error> {
        Ok(1)
    }

    async fn chat(
        &self,
        _agent_id: agy_bridge::agent::AgentId,
        content: &agy_bridge::content::Content,
    ) -> Result<agy_bridge::streaming::ChatResponseHandle, Error> {
        // Record the user message in history.
        let text = content.as_text().unwrap_or("(non-text)").to_owned();

        if let Ok(mut h) = self.history.lock() {
            h.push(agy_bridge::types::ConversationMessage {
                role: agy_bridge::types::MessageRole::User,
                content: text,
            });
        }

        self.turn_count.fetch_add(1, Ordering::SeqCst);

        // Create a channel pair and send a mock text response.
        let (writer, handle) = agy_bridge::streaming::channel();
        tokio::spawn(async move {
            writer
                .send_text("Mock response".to_owned())
                .await
                .expect("send_text");
            // writer is dropped here, closing the channel
        });

        Ok(handle)
    }

    async fn shutdown_agent(&self, _agent_id: agy_bridge::agent::AgentId) -> Result<(), Error> {
        Ok(())
    }

    async fn cancel(&self, _agent_id: agy_bridge::agent::AgentId) -> Result<(), Error> {
        Ok(())
    }

    async fn wait_for_idle(&self, _agent_id: agy_bridge::agent::AgentId) -> Result<(), Error> {
        Ok(())
    }

    async fn send(
        &self,
        _agent_id: agy_bridge::agent::AgentId,
        _content: &agy_bridge::content::Content,
    ) -> Result<(), Error> {
        Ok(())
    }

    async fn signal_idle(&self, _agent_id: agy_bridge::agent::AgentId) -> Result<(), Error> {
        Ok(())
    }

    async fn wait_for_wakeup(
        &self,
        _agent_id: agy_bridge::agent::AgentId,
        _timeout: std::time::Duration,
    ) -> Result<bool, Error> {
        Ok(false)
    }

    async fn wait_for_quota(&self) {}

    fn quota_registry(&self) -> &agy_bridge::quota::QuotaRegistry {
        &self.quota_registry
    }

    async fn record_quota_hit(&self, _retry_after: std::time::Duration) {}

    async fn history(
        &self,
        _agent_id: agy_bridge::agent::AgentId,
    ) -> Result<Vec<agy_bridge::types::ConversationMessage>, Error> {
        let h = self.history.lock().unwrap();
        Ok(h.clone())
    }

    async fn turn_count(&self, _agent_id: agy_bridge::agent::AgentId) -> Result<u32, Error> {
        Ok(self.turn_count.load(Ordering::SeqCst))
    }

    async fn total_usage(
        &self,
        _agent_id: agy_bridge::agent::AgentId,
    ) -> Result<agy_bridge::types::UsageMetadata, Error> {
        Ok(agy_bridge::types::UsageMetadata {
            prompt_token_count: Some(500),
            cached_content_token_count: None,
            candidates_token_count: Some(200),
            thoughts_token_count: Some(100),
            total_token_count: Some(800),
        })
    }

    async fn last_turn_usage(
        &self,
        _agent_id: agy_bridge::agent::AgentId,
    ) -> Result<agy_bridge::types::UsageMetadata, Error> {
        Ok(agy_bridge::types::UsageMetadata {
            prompt_token_count: Some(100),
            cached_content_token_count: None,
            candidates_token_count: Some(50),
            thoughts_token_count: Some(20),
            total_token_count: Some(170),
        })
    }

    async fn clear_history(&self, _agent_id: agy_bridge::agent::AgentId) -> Result<(), Error> {
        if let Ok(mut h) = self.history.lock() {
            h.clear();
        }
        self.turn_count.store(0, Ordering::SeqCst);
        Ok(())
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

/// Verify that `shutdown()` takes `&self` (non-consuming). After shutdown,
/// `is_started()` returns false — the agent is no longer usable for chat.
#[tokio::test]
async fn test_agent_shutdown_returns_ok() {
    let runtime = Arc::new(MockRuntime::new());
    let config = AgentConfig::default();

    let agent = agy_bridge::agent::AgentHandle::new(runtime, config, None, None, None)
        .await
        .unwrap();

    assert!(agent.is_started());

    // shutdown() takes &self, so the handle is still accessible after.
    agent.shutdown().await.unwrap();

    // After shutdown, the agent should no longer be considered started.
    assert!(!agent.is_started());
}

/// Verify that creating an agent with a configuration produces a valid handle.
#[tokio::test]
async fn test_agent_creation_with_config() {
    let runtime = Arc::new(MockRuntime::new());
    let config = AgentConfig::default();

    let agent = agy_bridge::agent::AgentHandle::new(runtime, config, None, None, None)
        .await
        .unwrap();

    assert!(agent.is_started());
    assert!(agent.id() > 0);
    assert!(agent.conversation_id().is_none());

    agent.shutdown().await.unwrap();
}

/// Verify a basic mock chat flow: send a message, get a response, check
/// history and turn count are updated.
#[tokio::test]
async fn test_basic_mock_chat_flow() {
    let runtime = Arc::new(MockRuntime::new());
    let config = AgentConfig::default();

    let agent = agy_bridge::agent::AgentHandle::new(Arc::clone(&runtime), config, None, None, None)
        .await
        .unwrap();

    // Turn count should start at 0.
    let count_before = agent.turn_count().await.unwrap();
    assert_eq!(count_before, 0);

    // Send a chat message and drain the response.
    let text = agent
        .chat("Hello, agent!")
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(text, "Mock response");

    // Turn count should have incremented.
    let count_after = agent.turn_count().await.unwrap();
    assert_eq!(count_after, 1);

    // History should contain the user message.
    let history = agent.history().await.unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].content, "Hello, agent!");

    // Usage metadata should be available.
    let usage = agent.total_usage().await.unwrap();
    assert_eq!(usage.total_token_count, Some(800));

    let last_usage = agent.last_turn_usage().await.unwrap();
    assert_eq!(last_usage.total_token_count, Some(170));

    // Clear history and verify reset.
    agent.clear_history().await.unwrap();
    let count_cleared = agent.turn_count().await.unwrap();
    assert_eq!(count_cleared, 0);
    let history_cleared = agent.history().await.unwrap();
    assert!(history_cleared.is_empty());

    agent.shutdown().await.unwrap();
}

/// Verify that the conversation ID is correctly restored from config on creation.
#[tokio::test]
async fn test_conversation_id_state_restoration() {
    let runtime = Arc::new(MockRuntime::new());
    let config = AgentConfig::builder()
        .conversation_id("existing-session-123")
        .build();

    let agent = agy_bridge::agent::AgentHandle::new(runtime, config, None, None, None)
        .await
        .unwrap();

    assert_eq!(
        agent.conversation_id(),
        Some("existing-session-123".to_owned())
    );
    agent.shutdown().await.unwrap();
}

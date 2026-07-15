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
}

impl MockRuntime {
    fn new() -> Self {
        Self {
            history: Mutex::new(Vec::new()),
            turn_count: AtomicU32::new(0),
        }
    }
}

impl agy_bridge::agent::Runtime for MockRuntime {
    async fn create_agent(
        &self,
        agent_id: u64,
        _config: AgentConfig,
    ) -> Result<(agy_bridge::agent::AgentId, Vec<agy_bridge::AvailableTool>), Error> {
        Ok((
            agent_id,
            vec![agy_bridge::AvailableTool {
                name: "mock_tool".to_owned(),
                description: "A mock tool for testing.".to_owned(),
                parameter_schema: serde_json::Value::Null,
                source: agy_bridge::ToolSource::Custom,
            }],
        ))
    }

    async fn chat(
        &self,
        _agent_id: agy_bridge::agent::AgentId,
        content: &agy_bridge::content::Content,
    ) -> Result<agy_bridge::streaming::ChatResponseHandle, Error> {
        // Record the user message in history.
        let text = content.as_text().unwrap_or("(non-text)").to_owned();

        let mut h = self
            .history
            .lock()
            .expect("history mutex poisoned in test mock");
        h.push(agy_bridge::types::ConversationMessage {
            role: agy_bridge::types::MessageRole::User,
            content: text,
        });

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
        let mut h = self
            .history
            .lock()
            .expect("history mutex poisoned in test mock");
        h.clear();
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

/// Verify that `available_tools()` returns the tool names from the runtime.
#[tokio::test]
async fn test_available_tools_from_runtime() {
    let runtime = Arc::new(MockRuntime::new());
    let config = AgentConfig::default();

    let agent = agy_bridge::agent::AgentHandle::new(runtime, config, None, None, None)
        .await
        .unwrap();

    let tools = agent.available_tools();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "mock_tool");
    assert_eq!(tools[0].source, agy_bridge::ToolSource::Custom);
    assert!(!tools[0].description.is_empty());

    let names = agent.available_tool_names();
    assert_eq!(names, vec!["mock_tool"]);

    agent.shutdown().await.unwrap();
}

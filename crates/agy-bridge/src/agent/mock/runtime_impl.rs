//! `Runtime` trait implementation for [`ToolAwareMockRuntime`].

use std::{sync::atomic::Ordering, time::Duration};

use super::ToolAwareMockRuntime;
use crate::{
    agent::{AgentId, Runtime},
    config::AgentConfig,
    content::Content,
    error::Error,
    streaming::{self, ChatResponseHandle},
    types::{ConversationMessage, MessageRole, UsageMetadata},
};

impl Runtime for ToolAwareMockRuntime {
    async fn create_agent(
        &self,
        agent_id: u64,
        _config: AgentConfig,
    ) -> Result<(AgentId, Vec<crate::tools::AvailableTool>), Error> {
        match self.last_create_id.lock() {
            Ok(mut slot) => *slot = Some(agent_id),
            Err(e) => tracing::error!("mock last_create_id mutex poisoned: {e}"),
        }
        if self.fail_create.load(Ordering::SeqCst) {
            return Err(Error::BackendError {
                message: "invalid config: missing system instructions".to_owned(),
            });
        }
        let id = agent_id;
        let tools = vec![
            crate::tools::AvailableTool {
                name: "run_command".to_owned(),
                description: "Execute a shell command.".to_owned(),
                parameter_schema: serde_json::Value::Null,
                source: crate::tools::ToolSource::Builtin,
            },
            crate::tools::AvailableTool {
                name: "view_file".to_owned(),
                description: "Read file contents.".to_owned(),
                parameter_schema: serde_json::Value::Null,
                source: crate::tools::ToolSource::Builtin,
            },
            crate::tools::AvailableTool {
                name: "add_numbers".to_owned(),
                description: "Adds two numbers together.".to_owned(),
                parameter_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "x": {"type": "integer"},
                        "y": {"type": "integer"}
                    },
                    "required": ["x", "y"]
                }),
                source: crate::tools::ToolSource::Custom,
            },
        ];
        Ok((id, tools))
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

    async fn remove_last_turn(&self, agent_id: AgentId) -> Result<(), Error> {
        let mut counts = self.chat_count.lock().unwrap();
        let entry = counts.entry(agent_id).or_insert(0);
        *entry = entry.saturating_sub(1);
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

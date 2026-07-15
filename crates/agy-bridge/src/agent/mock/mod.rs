//! Mock runtime for agent unit testing.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use super::{AgentHandle, AgentId};
use crate::{
    config::AgentConfig,
    error::Error,
    types::{ConversationMessage, MessageRole, UsageMetadata},
};

mod runtime_impl;

#[cfg(test)]
mod tests_conversation;
#[cfg(test)]
mod tests_lifecycle;

/// A mock runtime that simulates tool calls on the first `chat()` call
/// and returns text on subsequent calls, enabling tests of the `chat_text()`
/// agentic loop without a live Python runtime.
pub struct ToolAwareMockRuntime {
    /// Counts how many times `chat()` has been called per agent.
    /// First call → tool call; subsequent calls → text response.
    chat_count: std::sync::Mutex<std::collections::HashMap<AgentId, u32>>,
    /// If true, `create_agent` will fail.
    fail_create: AtomicBool,
    /// If true, first `chat` will return `QuotaExceeded` (then resets).
    fail_quota: AtomicBool,
    /// Tracks whether `try_shutdown_agent` was called (from Drop).
    pub(crate) try_shutdown_called: AtomicBool,
    /// Records the last `agent_id` passed to `create_agent`, so tests can
    /// assert cleanup of per-agent init state even on the create-failure path
    /// (where no `AgentHandle` — and thus no `id()` — is returned).
    pub(crate) last_create_id: std::sync::Mutex<Option<u64>>,
    /// Per-runtime quota registry.
    quota_registry: crate::quota::QuotaRegistry,
}

impl ToolAwareMockRuntime {
    pub(crate) fn new() -> Self {
        Self {
            chat_count: std::sync::Mutex::new(std::collections::HashMap::new()),
            fail_create: AtomicBool::new(false),
            fail_quota: AtomicBool::new(false),
            try_shutdown_called: AtomicBool::new(false),
            last_create_id: std::sync::Mutex::new(None),
            quota_registry: crate::quota::QuotaRegistry::new(),
        }
    }

    pub(crate) fn with_create_failure() -> Self {
        let rt = Self::new();
        rt.fail_create.store(true, Ordering::SeqCst);
        rt
    }
}

/// Shared test helper: builds a default [`AgentConfig`] for the mock tests.
#[cfg(test)]
fn test_config() -> AgentConfig {
    AgentConfig::default()
}

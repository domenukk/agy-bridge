//! Per-agent bridge state registry.
//!
//! Stores per-agent sidecar data (tool registries, hook runners, policy sets)
//! in a global `RwLock<HashMap>` keyed by agent ID. See the module-level docs
//! in `runtime/mod.rs` for the rationale behind global state.

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

#[cfg(feature = "python")]
/// Opaque agent identifier returned by the runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct AgentId(pub(crate) u64);

#[cfg(feature = "python")]
impl std::fmt::Display for AgentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "agent-{}", self.0)
    }
}

/// Process-global monotonic source of unique agent IDs.
///
/// A single counter shared by *all* bridges/runtimes guarantees that agent IDs
/// never collide across bridges, so per-agent entries in [`bridge_state()`]
/// (and the initializing-hook-runner registry) are always unambiguous.
static AGENT_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Allocate a new, process-globally-unique agent ID.
pub(crate) fn next_agent_id() -> u64 {
    AGENT_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// Per-agent state stored in the global [`BRIDGE_STATE`] registry.
///
/// Bundles all sidecar data that FFI callbacks need to look up by agent ID.
/// Consolidating into one struct means a single lock acquisition covers all
/// lookups/insertions/removals, preventing inconsistent partial state.
pub(crate) struct AgentBridgeState {
    /// Custom Rust tools registered for this agent.
    pub(crate) registry: Option<Arc<crate::tools::ToolRegistry>>,
    /// Lifecycle hooks for pre/post turn, tool-call gating, etc.
    pub(crate) hook_runner: Option<Arc<crate::hooks::Hooks>>,
    /// Policy rules governing tool-call permissions.
    pub(crate) policies: crate::policies::PolicySet,
    /// Interactive confirmation handler for `NeedsConfirmation` policies.
    pub(crate) policy_handler: Option<Arc<dyn crate::policies::AskUserHandler>>,
    /// Shared key-value state persisted across tool calls for this agent.
    pub(crate) tool_state: llm_tool::SharedState,
    /// The agent's conversation ID, sharing the same `Arc` as its
    /// [`AgentHandle`](crate::agent::AgentHandle) so updates are visible here
    /// immediately. Threaded into the [`ToolContext`](llm_tool::ToolContext)
    /// built for every custom-tool dispatch, so tools can identify which
    /// conversation they serve.
    ///
    /// It faithfully mirrors the SDK session: seeded from
    /// `AgentConfig::conversation_id` when resuming, then overwritten with the
    /// harness-assigned id during the first turn (synced from Python via
    /// `set_agent_conversation_id`). `None` until the harness assigns one for a
    /// fresh conversation.
    pub(crate) conversation_id: Arc<std::sync::Mutex<Option<String>>>,
    /// Structured payload of the most recent failed tool dispatch, if any.
    ///
    /// When a Rust tool returns `Err(ToolError)`, the error is serialized here
    /// (message + metadata) *before* being converted to a `PyErr`. The
    /// `on_tool_error` hook dispatch then takes this value to enrich
    /// [`OnToolErrorContext`](crate::hooks::OnToolErrorContext) with the same
    /// structured metadata that successful calls already receive — the SDK's
    /// Python callback only sees the model-facing message string, so the
    /// metadata cannot round-trip through Python.
    ///
    /// Cleared on the next successful dispatch to avoid surfacing a stale error.
    pub(crate) last_tool_error: std::sync::Mutex<Option<serde_json::Value>>,
}

/// Serialize a [`ToolError`](llm_tool::ToolError) and cache it in the agent's
/// [`last_tool_error`](AgentBridgeState::last_tool_error) slot.
///
/// Called from the tool dispatch error path. Failures to look up the agent or
/// serialize the error are logged (never silently swallowed) rather than
/// propagated, since the caller is about to raise the model-facing `PyErr`
/// regardless and losing metadata must not mask the original tool failure.
pub(crate) fn record_last_tool_error(agent_id: u64, error: &llm_tool::ToolError) {
    let value = match serde_json::to_value(error) {
        Ok(value) => value,
        Err(e) => {
            tracing::error!(
                agent_id,
                error = %e,
                "Failed to serialize ToolError for on_tool_error metadata — \
                 hook will receive no structured metadata"
            );
            return;
        }
    };
    with_last_tool_error_slot(agent_id, "record", |slot| *slot = Some(value));
}

/// Clear the agent's cached [`last_tool_error`](AgentBridgeState::last_tool_error),
/// preventing a previous failure from leaking into a later hook invocation.
pub(crate) fn clear_last_tool_error(agent_id: u64) {
    with_last_tool_error_slot(agent_id, "clear", |slot| *slot = None);
}

/// Remove and return the agent's cached
/// [`last_tool_error`](AgentBridgeState::last_tool_error), if present.
pub(crate) fn take_last_tool_error(agent_id: u64) -> Option<serde_json::Value> {
    let mut taken = None;
    with_last_tool_error_slot(agent_id, "take", |slot| taken = slot.take());
    taken
}

/// Run `f` against an agent's `last_tool_error` mutex, logging (rather than
/// panicking or silently dropping) if the registry lock or the slot mutex is
/// unavailable. `op` names the operation for diagnostics.
fn with_last_tool_error_slot(
    agent_id: u64,
    op: &str,
    f: impl FnOnce(&mut Option<serde_json::Value>),
) {
    let map = match bridge_state().read() {
        Ok(map) => map,
        Err(e) => {
            tracing::error!(
                agent_id,
                op,
                error = %e,
                "BRIDGE_STATE read lock poisoned — cannot access last_tool_error slot"
            );
            return;
        }
    };
    let Some(entry) = map.get(&agent_id) else {
        // No entry is normal during teardown races; debug-level only.
        tracing::debug!(
            agent_id,
            op,
            "No bridge state entry for last_tool_error access"
        );
        return;
    };
    match entry.last_tool_error.lock() {
        Ok(mut slot) => f(&mut slot),
        Err(e) => {
            tracing::error!(
                agent_id,
                op,
                error = %e,
                "last_tool_error mutex poisoned — structured error metadata unavailable"
            );
        }
    }
}

/// Single global registry of per-agent bridge state, keyed by agent ID.
///
/// # Lock choice
///
/// Uses `std::sync::RwLock` (not `tokio::sync::RwLock`) because the lock is
/// held only for brief `HashMap` insert/remove/lookup operations and is never
/// held across an `.await` point. This avoids the overhead of an async lock
/// and is safe from deadlocks.
///
/// # Scalability
///
/// For typical agent counts (< ~100), `RwLock<HashMap>` provides sufficient
/// throughput.  Read-side contention is bounded by the microsecond-scale lock
/// duration.  If the bridge ever needs to support thousands of concurrent
/// agents, replacing this with a `DashMap` would eliminate read-lock overhead
/// entirely — but is unnecessary for current workloads.
static BRIDGE_STATE: std::sync::OnceLock<
    std::sync::RwLock<std::collections::HashMap<u64, AgentBridgeState>>,
> = std::sync::OnceLock::new();

/// Access the global per-agent bridge state registry.
pub(crate) fn bridge_state()
-> &'static std::sync::RwLock<std::collections::HashMap<u64, AgentBridgeState>> {
    BRIDGE_STATE.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()))
}

/// Per-agent hook runners registered *before* the agent handle is fully
/// constructed, so that `on_session_start` hooks can fire during initialization.
static INITIALIZING_HOOK_RUNNERS: std::sync::OnceLock<
    std::sync::RwLock<std::collections::HashMap<u64, Arc<crate::hooks::Hooks>>>,
> = std::sync::OnceLock::new();

/// Access the per-agent initializing hook-runner registry.
pub(crate) fn initializing_hook_runners()
-> &'static std::sync::RwLock<std::collections::HashMap<u64, Arc<crate::hooks::Hooks>>> {
    INITIALIZING_HOOK_RUNNERS
        .get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()))
}

/// Store a conversation ID assigned to an agent.
pub(crate) fn set_agent_conversation_id(
    agent_id: u64,
    conversation_id: String,
) -> Result<(), crate::error::Error> {
    let map = bridge_state()
        .read()
        .map_err(|e| crate::error::Error::BackendError {
            message: format!("Failed to read BRIDGE_STATE: {e}"),
        })?;
    let Some(entry) = map.get(&agent_id) else {
        tracing::debug!(agent_id, "set_agent_conversation_id: no bridge state entry");
        return Ok(());
    };
    match entry.conversation_id.lock() {
        Ok(mut guard) => {
            if guard.as_deref() != Some(conversation_id.as_str()) {
                tracing::debug!(
                    agent_id,
                    conversation_id = %conversation_id,
                    "Storing conversation id"
                );
                *guard = Some(conversation_id);
            }
            Ok(())
        }
        Err(e) => {
            tracing::error!(
                agent_id,
                error = %e,
                "conversation_id mutex poisoned — conversation id not stored"
            );
            Err(crate::error::Error::BackendError {
                message: "conversation_id mutex poisoned".to_string(),
            })
        }
    }
}

/// Look up the current conversation ID for an agent, if set.
#[cfg(feature = "native")]
pub(crate) fn get_agent_conversation_id(agent_id: u64) -> Option<String> {
    let map = match bridge_state().read() {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(agent_id, error = %e, "get_agent_conversation_id: bridge_state lock poisoned");
            return None;
        }
    };
    let entry = map.get(&agent_id)?;
    match entry.conversation_id.lock() {
        Ok(guard) => guard.clone(),
        Err(e) => {
            tracing::warn!(agent_id, error = %e, "get_agent_conversation_id: conversation_id mutex poisoned");
            None
        }
    }
}

/// Evaluates policies and registered handlers to check if a tool execution is allowed.
pub(crate) fn check_tool_execution_allowed(
    agent_id: u64,
    name: &str,
    args_json: &str,
) -> Result<bool, crate::error::Error> {
    let map = bridge_state()
        .read()
        .map_err(|e| crate::error::Error::BackendError {
            message: format!("Failed to read BRIDGE_STATE: {e}"),
        })?;

    let Some(state) = map.get(&agent_id) else {
        return Err(crate::error::Error::BackendError {
            message: format!(
                "Agent {agent_id} not found in bridge state — it may have been shut down"
            ),
        });
    };

    let (is_allowed, needs_confirm) = match state.policies.evaluate(name) {
        crate::policies::PolicyDecision::Allow => (true, false),
        crate::policies::PolicyDecision::Deny => (false, false),
        crate::policies::PolicyDecision::NeedsConfirmation { .. } => (false, true),
    };

    if is_allowed {
        return Ok(true);
    }

    if needs_confirm && let Some(ref handler) = state.policy_handler {
        let handler = std::sync::Arc::clone(handler);
        // Drop the lock before calling the handler (it may block).
        drop(map);
        let args_val: serde_json::Value =
            serde_json::from_str(args_json).map_err(|e| crate::error::Error::BackendError {
                message: format!("Failed to parse policy args JSON: {e}"),
            })?;
        return Ok(handler.confirm(name, &args_val));
    }

    Ok(false)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    /// Sequential allocations must never repeat an ID.
    #[test]
    fn next_agent_id_is_unique_sequentially() {
        const N: usize = 1000;
        let ids: Vec<u64> = std::iter::repeat_with(next_agent_id).take(N).collect();
        let unique: HashSet<u64> = ids.iter().copied().collect();
        assert_eq!(unique.len(), N, "sequential IDs must all be unique");
    }

    /// Concurrent allocations across many threads must never hand out the same
    /// ID twice — the invariant that guarantees per-agent state entries (and
    /// initializing hook runners) never collide across bridges.
    #[test]
    fn next_agent_id_is_unique_under_concurrency() {
        const THREADS: usize = 16;
        const PER_THREAD: usize = 500;

        let handles: Vec<_> = std::iter::repeat_with(|| {
            std::thread::spawn(|| {
                std::iter::repeat_with(next_agent_id)
                    .take(PER_THREAD)
                    .collect::<Vec<u64>>()
            })
        })
        .take(THREADS)
        .collect();

        let mut all_ids = Vec::with_capacity(THREADS * PER_THREAD);
        for handle in handles {
            all_ids.extend(handle.join().expect("allocator thread must not panic"));
        }

        let unique: HashSet<u64> = all_ids.iter().copied().collect();
        assert_eq!(
            unique.len(),
            all_ids.len(),
            "concurrent allocations must be collision-free"
        );
    }
}

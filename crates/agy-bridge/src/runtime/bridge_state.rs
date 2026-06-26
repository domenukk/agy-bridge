//! Per-agent bridge state registry.
//!
//! Stores per-agent sidecar data (tool registries, hook runners, policy sets)
//! in a global `RwLock<HashMap>` keyed by agent ID. See the module-level docs
//! in `runtime/mod.rs` for the rationale behind global state.

use std::{collections::HashMap, sync::Arc};

/// Opaque agent identifier returned by the runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct AgentId(pub(crate) u64);

impl std::fmt::Display for AgentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "agent-{}", self.0)
    }
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
    pub(crate) tool_state: Arc<std::sync::RwLock<HashMap<String, serde_json::Value>>>,
    /// Shared conversation/session identifier synced from Python side.
    pub(crate) conversation_id: Arc<std::sync::Mutex<Option<String>>>,
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

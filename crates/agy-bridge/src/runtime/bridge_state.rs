//! Per-agent bridge state registry.
//!
//! Stores per-agent sidecar data (tool registries, hook runners, policy sets,
//! surrogate vaults, and `call_id`-keyed tool execution metadata) in a global
//! `RwLock<HashMap>` keyed by agent ID.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, OnceLock, RwLock,
        atomic::{AtomicU64, Ordering},
    },
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

const MAX_TRACKED_TOOL_CALL_SLOTS: usize = 256;
#[cfg(any(feature = "native", feature = "python"))]
const MAX_CONVERSATION_ID_LEN: usize = 128;

/// Validate that a conversation/cascade ID cannot traverse directories (`[A-H6]`).
///
/// # Errors
///
/// Returns [`Error::InvalidConfig`](crate::error::Error::InvalidConfig) if `id` is empty,
/// exceeds 128 bytes, or contains characters outside `[A-Za-z0-9_-]`.
#[cfg(any(feature = "native", feature = "python"))]
pub(crate) fn validate_conversation_id(id: &str) -> Result<(), crate::error::Error> {
    let is_safe = !id.is_empty()
        && id.len() <= MAX_CONVERSATION_ID_LEN
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if !is_safe {
        return Err(crate::error::Error::InvalidConfig {
            message: format!(
                "Invalid conversation_id {id:?}: must be 1..={MAX_CONVERSATION_ID_LEN} ASCII alphanumeric, '-', or '_' characters"
            ),
        });
    }
    Ok(())
}

/// Remove persisted conversation artifacts inside `save_dir` after validating `cascade_id`
/// against path traversal and refusing to follow directory symlinks.
#[cfg(any(feature = "native", feature = "python"))]
pub(crate) fn remove_persisted_conversation_files(
    save_dir: &std::path::Path,
    cascade_id: &str,
) -> Result<(), crate::error::Error> {
    validate_conversation_id(cascade_id)?;
    for candidate in [
        save_dir.join(format!("{cascade_id}.db")),
        save_dir.join(format!("{cascade_id}.db-wal")),
        save_dir.join(format!("{cascade_id}.db-shm")),
        save_dir.join(format!("traj-{cascade_id}")),
        save_dir.join(cascade_id),
    ] {
        let Ok(meta) = std::fs::symlink_metadata(&candidate) else {
            continue;
        };
        if meta.file_type().is_dir() && !meta.file_type().is_symlink() {
            if let Err(e) = std::fs::remove_dir_all(&candidate) {
                tracing::debug!(path = %candidate.display(), error = %e, "Failed removing persisted conversation dir");
            }
        } else if let Err(e) = std::fs::remove_file(&candidate) {
            tracing::debug!(path = %candidate.display(), error = %e, "Failed removing persisted conversation file");
        }
    }
    Ok(())
}

/// Process-global monotonic source of unique agent IDs.
static AGENT_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Allocate a new, process-globally-unique agent ID.
#[must_use]
pub fn next_agent_id() -> u64 {
    AGENT_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// Bounded `call_id`-keyed metadata store with an unnamed fallback slot for
/// callers or runtimes that do not supply a tool call ID.
pub(crate) struct TrackedToolMap<T> {
    by_call_id: HashMap<String, T>,
    latest_fallback: Option<T>,
}

impl<T> Default for TrackedToolMap<T> {
    fn default() -> Self {
        Self {
            by_call_id: HashMap::new(),
            latest_fallback: None,
        }
    }
}

impl<T> TrackedToolMap<T> {
    fn insert(&mut self, call_id: &str, value: T) {
        if call_id.is_empty() {
            self.latest_fallback = Some(value);
            return;
        }
        if self.by_call_id.len() >= MAX_TRACKED_TOOL_CALL_SLOTS
            && !self.by_call_id.contains_key(call_id)
            && let Some(stale_key) = self.by_call_id.keys().next().cloned()
        {
            self.by_call_id.remove(&stale_key);
        }
        self.by_call_id.insert(call_id.to_owned(), value);
    }

    fn take(&mut self, call_id: &str) -> Option<T> {
        if call_id.is_empty() {
            return self.latest_fallback.take();
        }
        self.by_call_id
            .remove(call_id)
            .or_else(|| self.latest_fallback.take())
    }

    fn clear(&mut self, call_id: &str) {
        if call_id.is_empty() {
            self.latest_fallback = None;
        } else {
            self.by_call_id.remove(call_id);
        }
    }
}

/// Per-agent state stored in the global [`BRIDGE_STATE`] registry.
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
    /// Per-agent host-pinned surrogate vault (`<surrogate:kind:id>`).
    pub(crate) surrogate_vault: llm_tool::SurrogateVault,
    /// Minimum prompt-injection sanitization floor configured for this agent.
    pub(crate) tool_output_floor: crate::tools::OutputSanitization,
    /// The agent's conversation ID, sharing the same `Arc` as its [`AgentHandle`](crate::agent::AgentHandle).
    pub(crate) conversation_id: Arc<Mutex<Option<String>>>,
    /// Structured payloads of failed custom tool dispatches, keyed by `call_id`.
    pub(crate) tool_errors: Mutex<TrackedToolMap<CapturedToolError>>,
    /// Cached structured outputs of successful custom tool dispatches, keyed by `call_id`.
    pub(crate) tool_outputs: Mutex<TrackedToolMap<CapturedToolOutput>>,
    /// Custom tool invocations already approved via `PolicyDecisionRequest` / `_rust_confirm_handler`,
    /// consumed once by `check_tool_execution_allowed` if `TransformToolInput` did not mutate `args`.
    pub(crate) confirmed_tool_calls: Mutex<Vec<ConfirmedToolCall>>,
    /// Structured output restored from persisted `InitializeConversationResponse.history` on session resume.
    pub(crate) restored_structured_output: Mutex<Option<serde_json::Value>>,
    /// Tool `call_id`s already evaluated via `CallHookRequest(PreTool)` so `run_tool_pre_check`
    /// never executes `TransformToolInput` / `PreToolCallDecide` twice on the same call.
    #[cfg(feature = "native")]
    pub(crate) pre_tool_evaluated_calls: Mutex<std::collections::HashSet<String>>,
    /// Ensures `OnSessionStart` fires at most once per agent on `NativeRuntime`.
    #[cfg(feature = "native")]
    pub(crate) session_start_fired: std::sync::atomic::AtomicBool,
    /// Ensures `OnSessionEnd` fires at most once per agent on `NativeRuntime`.
    #[cfg(feature = "native")]
    pub(crate) session_end_fired: std::sync::atomic::AtomicBool,
}

/// Snapshot of an interactive `AskUser` approval granted by `PolicyDecisionRequest` or Python policy confirm.
pub(crate) struct ConfirmedToolCall {
    pub(crate) tool_name: String,
    pub(crate) args: serde_json::Value,
}

fn json_values_semantically_equal(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    match (a, b) {
        (serde_json::Value::Null, serde_json::Value::Null) => true,
        (serde_json::Value::Bool(x), serde_json::Value::Bool(y)) => x == y,
        (serde_json::Value::String(x), serde_json::Value::String(y)) => x == y,
        (serde_json::Value::Number(x), serde_json::Value::Number(y)) => {
            x == y || (x.as_f64().is_some() && x.as_f64() == y.as_f64())
        }
        (serde_json::Value::Array(xs), serde_json::Value::Array(ys)) => {
            xs.len() == ys.len()
                && xs
                    .iter()
                    .zip(ys.iter())
                    .all(|(x, y)| json_values_semantically_equal(x, y))
        }
        (serde_json::Value::Object(xm), serde_json::Value::Object(ym)) => {
            xm.len() == ym.len()
                && xm.iter().all(|(k, xv)| {
                    ym.get(k)
                        .is_some_and(|yv| json_values_semantically_equal(xv, yv))
                })
        }
        _ => false,
    }
}

/// Record that `PolicyDecisionRequest` / `_rust_confirm_handler` approved `(tool_name, args)` for `agent_id`.
#[cfg(any(feature = "native", feature = "python"))]
pub(crate) fn record_confirmed_tool_call(agent_id: u64, tool_name: &str, args: serde_json::Value) {
    let map = match bridge_state().read() {
        Ok(map) => map,
        Err(e) => {
            tracing::error!(agent_id, error = %e, "BRIDGE_STATE read lock poisoned in record_confirmed_tool_call");
            return;
        }
    };
    let Some(entry) = map.get(&agent_id) else {
        return;
    };
    match entry.confirmed_tool_calls.lock() {
        Ok(mut list) => {
            if list.len() >= MAX_TRACKED_TOOL_CALL_SLOTS {
                list.remove(0);
            }
            list.push(ConfirmedToolCall {
                tool_name: tool_name.to_owned(),
                args,
            });
        }
        Err(e) => {
            tracing::error!(agent_id, error = %e, "confirmed_tool_calls mutex poisoned");
        }
    }
}

/// Invalidate one cached `AskUser` approval for `(tool_name, orig_args)` when `TransformToolInput` mutates `args`.
#[cfg(any(feature = "native", feature = "python"))]
pub(crate) fn invalidate_confirmed_tool_call(
    agent_id: u64,
    tool_name: &str,
    orig_args: &serde_json::Value,
) {
    let map = match bridge_state().read() {
        Ok(map) => map,
        Err(e) => {
            tracing::error!(agent_id, error = %e, "BRIDGE_STATE read lock poisoned in invalidate_confirmed_tool_call");
            return;
        }
    };
    let Some(entry) = map.get(&agent_id) else {
        return;
    };
    match entry.confirmed_tool_calls.lock() {
        Ok(mut list) => {
            if let Some(pos) = list.iter().position(|c| {
                c.tool_name == tool_name && json_values_semantically_equal(&c.args, orig_args)
            }) {
                list.remove(pos);
            }
        }
        Err(e) => {
            tracing::error!(agent_id, error = %e, "confirmed_tool_calls mutex poisoned");
        }
    }
}

fn take_confirmed_tool_call(
    confirmed_slot: &Mutex<Vec<ConfirmedToolCall>>,
    tool_name: &str,
    args: &serde_json::Value,
) -> bool {
    match confirmed_slot.lock() {
        Ok(mut list) => {
            if let Some(pos) = list.iter().position(|c| {
                c.tool_name == tool_name && json_values_semantically_equal(&c.args, args)
            }) {
                list.remove(pos);
                true
            } else {
                false
            }
        }
        Err(e) => {
            tracing::error!(error = %e, "confirmed_tool_calls mutex poisoned in take_confirmed_tool_call");
            false
        }
    }
}

/// Structured snapshot of a successful custom tool dispatch.
#[derive(Debug, Clone, PartialEq)]
pub struct CapturedToolOutput {
    /// Arguments the tool was invoked with.
    pub tool_args: serde_json::Value,
    /// Structured [`ToolOutput`](llm_tool::ToolOutput) metadata (`Null` when empty).
    pub metadata: serde_json::Value,
}

/// Cache `(tool_args, metadata)` from a successful custom tool call under `call_id`.
pub fn record_tool_output_for_call(
    agent_id: u64,
    call_id: &str,
    tool_args: serde_json::Value,
    metadata: serde_json::Value,
) {
    with_tool_outputs_slot(agent_id, "record", |map| {
        map.insert(
            call_id,
            CapturedToolOutput {
                tool_args,
                metadata,
            },
        );
    });
}

/// Remove and return the cached [`CapturedToolOutput`] for `call_id`, if present.
#[must_use]
pub fn take_tool_output_for_call(agent_id: u64, call_id: &str) -> Option<CapturedToolOutput> {
    let mut taken = None;
    with_tool_outputs_slot(agent_id, "take", |map| taken = map.take(call_id));
    taken
}

/// Cache `(tool_args, metadata)` from a successful custom tool call in the fallback slot.
pub fn record_last_tool_output(
    agent_id: u64,
    tool_args: serde_json::Value,
    metadata: serde_json::Value,
) {
    record_tool_output_for_call(agent_id, "", tool_args, metadata);
}

/// Remove and return the agent's fallback cached [`CapturedToolOutput`], if present.
#[must_use]
pub fn take_last_tool_output(agent_id: u64) -> Option<CapturedToolOutput> {
    take_tool_output_for_call(agent_id, "")
}

fn with_tool_outputs_slot(
    agent_id: u64,
    op: &str,
    f: impl FnOnce(&mut TrackedToolMap<CapturedToolOutput>),
) {
    let map = match bridge_state().read() {
        Ok(map) => map,
        Err(e) => {
            tracing::error!(
                agent_id,
                op,
                error = %e,
                "BRIDGE_STATE read lock poisoned — cannot access tool_outputs"
            );
            return;
        }
    };
    let Some(entry) = map.get(&agent_id) else {
        tracing::debug!(
            agent_id,
            op,
            "No bridge state entry for tool_outputs access"
        );
        return;
    };
    match entry.tool_outputs.lock() {
        Ok(mut slot) => f(&mut slot),
        Err(e) => {
            tracing::error!(
                agent_id,
                op,
                error = %e,
                "tool_outputs mutex poisoned — structured output metadata unavailable"
            );
        }
    }
}

/// Structured snapshot of a failed custom tool dispatch.
#[derive(Debug, Clone, PartialEq)]
pub struct CapturedToolError {
    /// Arguments the failing tool was invoked with (`Null` when unknown).
    pub tool_args: serde_json::Value,
    /// Structured [`ToolError`](llm_tool::ToolError) metadata, or `None` when empty.
    pub metadata: Option<serde_json::Map<String, serde_json::Value>>,
}

impl CapturedToolError {
    /// Capture `error` (and the arguments that produced it) directly from [`llm_tool::ToolError::metadata`].
    #[must_use]
    pub fn from_tool_error(tool_args: serde_json::Value, error: &llm_tool::ToolError) -> Self {
        let metadata = (!error.metadata().is_empty()).then(|| {
            error
                .metadata()
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        });
        Self {
            tool_args,
            metadata,
        }
    }
}

/// Snapshot a [`ToolError`](llm_tool::ToolError) under `call_id`.
pub fn record_tool_error_for_call(
    agent_id: u64,
    call_id: &str,
    tool_args: serde_json::Value,
    error: &llm_tool::ToolError,
) {
    let captured = CapturedToolError::from_tool_error(tool_args, error);
    with_tool_errors_slot(agent_id, "record", |map| map.insert(call_id, captured));
}

/// Clear any cached [`CapturedToolError`] for `call_id`.
pub fn clear_tool_error_for_call(agent_id: u64, call_id: &str) {
    with_tool_errors_slot(agent_id, "clear", |map| map.clear(call_id));
}

/// Remove and return the cached [`CapturedToolError`] for `call_id`, if present.
#[must_use]
pub fn take_tool_error_for_call(agent_id: u64, call_id: &str) -> Option<CapturedToolError> {
    let mut taken = None;
    with_tool_errors_slot(agent_id, "take", |map| taken = map.take(call_id));
    taken
}

/// Snapshot a [`ToolError`](llm_tool::ToolError) in the agent's fallback slot.
pub fn record_last_tool_error(
    agent_id: u64,
    tool_args: serde_json::Value,
    error: &llm_tool::ToolError,
) {
    record_tool_error_for_call(agent_id, "", tool_args, error);
}

/// Clear the agent's fallback cached [`CapturedToolError`].
pub fn clear_last_tool_error(agent_id: u64) {
    clear_tool_error_for_call(agent_id, "");
}

/// Remove and return the agent's fallback cached [`CapturedToolError`], if present.
#[must_use]
pub fn take_last_tool_error(agent_id: u64) -> Option<CapturedToolError> {
    take_tool_error_for_call(agent_id, "")
}

fn with_tool_errors_slot(
    agent_id: u64,
    op: &str,
    f: impl FnOnce(&mut TrackedToolMap<CapturedToolError>),
) {
    let map = match bridge_state().read() {
        Ok(map) => map,
        Err(e) => {
            tracing::error!(
                agent_id,
                op,
                error = %e,
                "BRIDGE_STATE read lock poisoned — cannot access tool_errors"
            );
            return;
        }
    };
    let Some(entry) = map.get(&agent_id) else {
        tracing::debug!(agent_id, op, "No bridge state entry for tool_errors access");
        return;
    };
    match entry.tool_errors.lock() {
        Ok(mut slot) => f(&mut slot),
        Err(e) => {
            tracing::error!(
                agent_id,
                op,
                error = %e,
                "tool_errors mutex poisoned — structured error metadata unavailable"
            );
        }
    }
}

/// Record that `CallHookRequest(PreTool)` has already evaluated `call_id` for `agent_id`.
#[cfg(feature = "native")]
pub(crate) fn mark_pre_tool_evaluated(agent_id: u64, call_id: &str) {
    if call_id.is_empty() {
        return;
    }
    let map = match bridge_state().read() {
        Ok(map) => map,
        Err(e) => {
            tracing::error!(agent_id, error = %e, "BRIDGE_STATE read lock poisoned in mark_pre_tool_evaluated");
            return;
        }
    };
    let Some(entry) = map.get(&agent_id) else {
        return;
    };
    match entry.pre_tool_evaluated_calls.lock() {
        Ok(mut set) => {
            if set.len() >= MAX_TRACKED_TOOL_CALL_SLOTS
                && !set.contains(call_id)
                && let Some(stale) = set.iter().next().cloned()
            {
                set.remove(&stale);
            }
            set.insert(call_id.to_owned());
        }
        Err(e) => {
            tracing::error!(agent_id, error = %e, "pre_tool_evaluated_calls mutex poisoned");
        }
    }
}

/// Consume whether `call_id` was already evaluated by `CallHookRequest(PreTool)`.
#[cfg(feature = "native")]
#[must_use]
pub(crate) fn take_pre_tool_evaluated(agent_id: u64, call_id: &str) -> bool {
    if call_id.is_empty() {
        return false;
    }
    let map = match bridge_state().read() {
        Ok(map) => map,
        Err(e) => {
            tracing::error!(agent_id, error = %e, "BRIDGE_STATE read lock poisoned in take_pre_tool_evaluated");
            return false;
        }
    };
    let Some(entry) = map.get(&agent_id) else {
        return false;
    };
    match entry.pre_tool_evaluated_calls.lock() {
        Ok(mut set) => set.remove(call_id),
        Err(e) => {
            tracing::error!(agent_id, error = %e, "pre_tool_evaluated_calls mutex poisoned");
            false
        }
    }
}

static BRIDGE_STATE: OnceLock<RwLock<HashMap<u64, AgentBridgeState>>> = OnceLock::new();

/// Access the global per-agent bridge state registry.
pub(crate) fn bridge_state() -> &'static RwLock<HashMap<u64, AgentBridgeState>> {
    BRIDGE_STATE.get_or_init(|| RwLock::new(HashMap::new()))
}

static INITIALIZING_HOOK_RUNNERS: OnceLock<RwLock<HashMap<u64, Arc<crate::hooks::Hooks>>>> =
    OnceLock::new();

/// Access the per-agent initializing hook-runner registry.
pub(crate) fn initializing_hook_runners() -> &'static RwLock<HashMap<u64, Arc<crate::hooks::Hooks>>>
{
    INITIALIZING_HOOK_RUNNERS.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Store a conversation ID assigned to an agent.
///
/// # Errors
///
/// Returns [`Error::BackendError`](crate::error::Error::BackendError) if the
/// bridge-state `RwLock` or per-agent `conversation_id` mutex is poisoned.
pub fn set_agent_conversation_id(
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
#[must_use]
pub fn get_agent_conversation_id(agent_id: u64) -> Option<String> {
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

/// Look up the custom [`ToolRegistry`](crate::tools::ToolRegistry) registered
/// for `agent_id`, if any.
#[must_use]
pub fn agent_registry(agent_id: u64) -> Option<Arc<crate::tools::ToolRegistry>> {
    match bridge_state().read() {
        Ok(map) => map.get(&agent_id).and_then(|entry| entry.registry.clone()),
        Err(e) => {
            tracing::error!(
                agent_id,
                error = %e,
                "BRIDGE_STATE read lock poisoned in agent_registry"
            );
            None
        }
    }
}

/// Return the effective [`OutputSanitization`](crate::tools::OutputSanitization) floor
/// for `agent_id` (defaults to `SanitizeTokens` if the agent entry is absent or poisoned).
#[must_use]
pub fn agent_output_floor(agent_id: u64) -> crate::tools::OutputSanitization {
    match bridge_state().read() {
        Ok(map) => {
            map.get(&agent_id)
                .map_or(crate::tools::OutputSanitization::SanitizeTokens, |entry| {
                    entry
                        .registry
                        .as_ref()
                        .map_or(entry.tool_output_floor, |reg| reg.output_floor())
                })
        }
        Err(e) => {
            tracing::error!(
                agent_id,
                error = %e,
                "BRIDGE_STATE read lock poisoned in agent_output_floor; defaulting to SanitizeTokens"
            );
            crate::tools::OutputSanitization::SanitizeTokens
        }
    }
}

/// Apply `agent_id`'s [`OutputSanitization`](crate::tools::OutputSanitization) floor to a
/// model-facing error or `OnToolError` replacement string.
#[cfg(any(feature = "native", feature = "python"))]
#[must_use]
pub(crate) fn sanitize_with_agent_floor(agent_id: u64, raw: &str) -> String {
    agent_output_floor(agent_id).apply_str(raw).into_owned()
}

/// Look up the [`Hooks`](crate::hooks::Hooks) runner registered for `agent_id`,
/// if any.
#[must_use]
pub fn agent_hooks(agent_id: u64) -> Option<Arc<crate::hooks::Hooks>> {
    match bridge_state().read() {
        Ok(map) => {
            if let Some(hooks) = map
                .get(&agent_id)
                .and_then(|entry| entry.hook_runner.clone())
            {
                return Some(hooks);
            }
        }
        Err(e) => {
            tracing::error!(
                agent_id,
                error = %e,
                "BRIDGE_STATE read lock poisoned in agent_hooks"
            );
        }
    }
    match initializing_hook_runners().read() {
        Ok(init_map) => init_map.get(&agent_id).cloned(),
        Err(e) => {
            tracing::error!(
                agent_id,
                error = %e,
                "INITIALIZING_HOOK_RUNNERS read lock poisoned in agent_hooks"
            );
            None
        }
    }
}

/// Look up a clone of the [`PolicySet`](crate::policies::PolicySet) configured
/// for `agent_id`, if the agent is registered in bridge state.
#[must_use]
pub fn agent_policies(agent_id: u64) -> Option<crate::policies::PolicySet> {
    match bridge_state().read() {
        Ok(map) => map.get(&agent_id).map(|entry| entry.policies.clone()),
        Err(e) => {
            tracing::error!(
                agent_id,
                error = %e,
                "BRIDGE_STATE read lock poisoned in agent_policies"
            );
            None
        }
    }
}

/// Look up the interactive [`AskUserHandler`](crate::policies::AskUserHandler)
/// registered for `agent_id`, if any.
#[must_use]
pub fn agent_policy_handler(agent_id: u64) -> Option<Arc<dyn crate::policies::AskUserHandler>> {
    match bridge_state().read() {
        Ok(map) => map
            .get(&agent_id)
            .and_then(|entry| entry.policy_handler.clone()),
        Err(e) => {
            tracing::error!(
                agent_id,
                error = %e,
                "BRIDGE_STATE read lock poisoned in agent_policy_handler"
            );
            None
        }
    }
}

/// Look up the shared per-agent [`SharedState`](llm_tool::SharedState) used
/// across custom tool dispatches for `agent_id`, if registered.
#[must_use]
pub fn agent_tool_state(agent_id: u64) -> Option<llm_tool::SharedState> {
    match bridge_state().read() {
        Ok(map) => map.get(&agent_id).map(|entry| entry.tool_state.clone()),
        Err(e) => {
            tracing::error!(
                agent_id,
                error = %e,
                "BRIDGE_STATE read lock poisoned in agent_tool_state"
            );
            None
        }
    }
}

/// Store or clear the structured output restored from a resumed session's initial history.
pub(crate) fn set_restored_structured_output(agent_id: u64, value: Option<serde_json::Value>) {
    let Ok(map) = bridge_state().read() else {
        tracing::error!(
            agent_id,
            "BRIDGE_STATE read lock poisoned in set_restored_structured_output"
        );
        return;
    };
    let Some(entry) = map.get(&agent_id) else {
        return;
    };
    match entry.restored_structured_output.lock() {
        Ok(mut guard) => *guard = value,
        Err(poisoned) => *poisoned.into_inner() = value,
    }
}

/// Read the structured output restored from a resumed session's initial history, if any.
#[must_use]
pub(crate) fn agent_restored_structured_output(agent_id: u64) -> Option<serde_json::Value> {
    let Ok(map) = bridge_state().read() else {
        tracing::error!(
            agent_id,
            "BRIDGE_STATE read lock poisoned in agent_restored_structured_output"
        );
        return None;
    };
    let entry = map.get(&agent_id)?;
    match entry.restored_structured_output.lock() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

/// Atomically claim the one-shot `OnSessionStart` execution slot for `agent_id`.
#[cfg(feature = "native")]
#[must_use]
pub(crate) fn claim_session_start_once(agent_id: u64) -> bool {
    let Ok(map) = bridge_state().read() else {
        return true;
    };
    map.get(&agent_id)
        .is_none_or(|e| !e.session_start_fired.swap(true, Ordering::SeqCst))
}

/// Atomically claim the one-shot `OnSessionEnd` execution slot for `agent_id`.
#[cfg(feature = "native")]
#[must_use]
pub(crate) fn claim_session_end_once(agent_id: u64) -> bool {
    let Ok(map) = bridge_state().read() else {
        return true;
    };
    map.get(&agent_id)
        .is_none_or(|e| !e.session_end_fired.swap(true, Ordering::SeqCst))
}

/// Look up the shared per-agent [`SurrogateVault`](llm_tool::SurrogateVault) used
/// across custom tool dispatches for `agent_id`, if registered.
#[must_use]
pub fn agent_surrogate_vault(agent_id: u64) -> Option<llm_tool::SurrogateVault> {
    match bridge_state().read() {
        Ok(map) => map
            .get(&agent_id)
            .map(|entry| entry.surrogate_vault.clone()),
        Err(e) => {
            tracing::error!(
                agent_id,
                error = %e,
                "BRIDGE_STATE read lock poisoned in agent_surrogate_vault"
            );
            None
        }
    }
}

/// Remove `agent_id` from both the active bridge-state registry and the
/// initializing hook-runner registry.
pub fn remove_agent_bridge_state(agent_id: u64) {
    match bridge_state().write() {
        Ok(mut map) => {
            map.remove(&agent_id);
        }
        Err(e) => {
            tracing::error!(
                agent_id,
                error = %e,
                "BRIDGE_STATE write lock poisoned in remove_agent_bridge_state"
            );
        }
    }
    match initializing_hook_runners().write() {
        Ok(mut init_map) => {
            init_map.remove(&agent_id);
        }
        Err(e) => {
            tracing::error!(
                agent_id,
                error = %e,
                "INITIALIZING_HOOK_RUNNERS write lock poisoned in remove_agent_bridge_state"
            );
        }
    }
}

/// Evaluates policies and registered handlers to check if a tool execution is allowed.
///
/// # Errors
///
/// Returns [`Error::BackendError`](crate::error::Error::BackendError) if the
/// bridge-state lock is poisoned, `agent_id` is not registered, or `args_json`
/// fails JSON parsing when an [`AskUserHandler`](crate::policies::AskUserHandler)
/// must be consulted.
pub fn check_tool_execution_allowed(
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

    if needs_confirm {
        let args_val: serde_json::Value =
            serde_json::from_str(args_json).map_err(|e| crate::error::Error::BackendError {
                message: format!("Failed to parse policy args JSON: {e}"),
            })?;
        if take_confirmed_tool_call(&state.confirmed_tool_calls, name, &args_val) {
            return Ok(true);
        }
        if let Some(ref handler) = state.policy_handler {
            let handler = Arc::clone(handler);
            drop(map);
            return Ok(handler.confirm(name, &args_val));
        }
    }

    Ok(false)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn next_agent_id_is_unique_sequentially() {
        const N: usize = 1000;
        let ids: Vec<u64> = std::iter::repeat_with(next_agent_id).take(N).collect();
        let unique: HashSet<u64> = ids.iter().copied().collect();
        assert_eq!(unique.len(), N, "sequential IDs must all be unique");
    }

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

    #[test]
    fn tracked_tool_map_isolates_parallel_call_ids_and_fallback() {
        let mut map = TrackedToolMap::default();
        map.insert("call_a", 10);
        map.insert("call_b", 20);
        assert_eq!(map.take("call_a"), Some(10));
        assert_eq!(map.take("call_b"), Some(20));
        assert_eq!(map.take("call_a"), None);

        map.insert("", 99);
        assert_eq!(map.take("call_unmatched"), Some(99));
        assert_eq!(map.take(""), None);
    }

    #[test]
    fn agent_hooks_falls_back_to_initializing_registry_and_removes_cleanly() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let agent_id = next_agent_id();
        assert!(agent_hooks(agent_id).is_none());

        let fired = Arc::new(AtomicBool::new(false));
        let fired_clone = Arc::clone(&fired);
        let hooks = Arc::new(crate::hooks::Hooks::new().with_session_start(
            "init-test",
            move |_ctx| {
                fired_clone.store(true, Ordering::SeqCst);
            },
        ));

        initializing_hook_runners()
            .write()
            .expect("initializing_hook_runners write lock")
            .insert(agent_id, Arc::clone(&hooks));

        let resolved = agent_hooks(agent_id).expect("must resolve from initializing registry");
        resolved.run_on_session_start(&crate::hooks::OnSessionStartContext {
            session: crate::hooks::SessionContext {
                session_id: format!("agent-{agent_id}"),
                agent_id,
                started_at: std::time::SystemTime::UNIX_EPOCH,
            },
        });
        assert!(fired.load(Ordering::SeqCst));

        remove_agent_bridge_state(agent_id);
        assert!(agent_hooks(agent_id).is_none());
    }
}

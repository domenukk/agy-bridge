//! Hook bridge for the Antigravity SDK.
//!
//! Defines Rust-side hook types that wrap callbacks for agent lifecycle
//! hook points: pre-turn, post-turn, pre-tool-call-decide, post-tool-call,
//! compaction, session start/end, tool errors, user interactions, and
//! tool-input transformation.
//!
//! The actual Python wrapping (creating `PyO3` classes that the SDK dispatches to)
//! requires the Python runtime and is gated behind integration tests.

use std::time::Instant;

use serde::{Deserialize, Serialize};

/// Result of a hook decision (mirrors SDK `HookResult`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookResult {
    /// Whether execution should proceed.
    pub allow: bool,
    /// Optional explanation or response message.
    pub message: String,
}

impl HookResult {
    /// Create an "allow" result with an empty message.
    #[must_use]
    pub const fn allow() -> Self {
        Self {
            allow: true,
            message: String::new(),
        }
    }

    /// Create an "allow" result with a message.
    #[must_use]
    pub fn allow_with_message(message: impl Into<String>) -> Self {
        Self {
            allow: true,
            message: message.into(),
        }
    }

    /// Create a "deny" result with a reason.
    #[must_use]
    pub fn deny(reason: impl Into<String>) -> Self {
        Self {
            allow: false,
            message: reason.into(),
        }
    }
}

// ── Hook context structs ────────────────────────────────────────────────────

/// Persistent session metadata passed to session-lifecycle hooks.
///
/// Created when a session starts and carried through to session-end hooks
/// so hooks can correlate events, measure session duration, and identify
/// the agent instance.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionContext {
    /// Unique identifier for this session.
    pub session_id: String,
    /// Numeric agent identifier within the bridge runtime.
    pub agent_id: u64,
    /// Monotonic timestamp of when the session was started.
    #[serde(skip, default = "std::time::Instant::now")]
    pub started_at: Instant,
}

/// Context passed to [`HookPoint::OnSessionStart`] hooks.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OnSessionStartContext {
    /// Session metadata for the newly started session.
    pub session: SessionContext,
}

/// Context passed to [`HookPoint::OnSessionEnd`] hooks.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OnSessionEndContext {
    /// Session metadata for the ending session.
    pub session: SessionContext,
}

/// Context passed to [`HookPoint::OnCompaction`] hooks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnCompactionContext {}

/// Context passed to [`HookPoint::OnInteraction`] hooks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnInteractionContext {
    /// The interaction message content.
    pub message: String,
}

/// Context passed to [`HookPoint::PreTurn`] hooks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreTurnContext {
    /// The user prompt for this turn.
    pub prompt: String,
    /// The 1-based turn number.
    pub turn_number: u32,
}

/// Context passed to [`HookPoint::PostTurn`] hooks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostTurnContext {
    /// The model's response text for this turn.
    pub response_text: String,
    /// The 1-based turn number.
    pub turn_number: u32,
}

/// Context passed to [`HookPoint::PreToolCallDecide`] hooks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreToolCallDecideContext {
    /// Name of the tool about to be called.
    #[serde(alias = "name")]
    pub tool_name: String,
    /// Arguments the tool will receive.
    #[serde(alias = "args", default)]
    pub tool_args: serde_json::Value,
}

/// Context passed to [`HookPoint::PostToolCall`] hooks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostToolCallContext {
    /// Name of the tool that was called.
    #[serde(alias = "name")]
    pub tool_name: String,
    /// Arguments the tool received.
    #[serde(alias = "args", default)]
    pub tool_args: serde_json::Value,
    /// The tool's return value (serialised).
    pub result: String,
    /// Structured metadata from the tool response (if any).
    #[serde(default)]
    pub metadata: serde_json::Value,
}

/// Context passed to [`HookPoint::OnToolError`] hooks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnToolErrorContext {
    /// Name of the tool that errored.
    #[serde(alias = "name")]
    pub tool_name: String,
    /// Arguments the tool received.
    #[serde(alias = "args", default)]
    pub tool_args: serde_json::Value,
    /// The error message.
    pub error: String,
}

/// Identifies the point in the agent lifecycle where a hook fires.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HookPoint {
    /// Before the model processes a turn (receives the user prompt).
    PreTurn,
    /// After the model completes a turn (receives the model response).
    PostTurn,
    /// Before a tool call is executed — can approve or deny.
    PreToolCallDecide,
    /// After a tool call completes (receives the tool result).
    PostToolCall,
    /// Fires when the context window is compacted (trimmed to fit limits).
    OnCompaction,
    /// Fires when a new agent session begins.
    OnSessionStart,
    /// Fires when an agent session ends.
    OnSessionEnd,
    /// Fires when a tool call returns an error.
    OnToolError,
    /// Fires on each user interaction (message received from user).
    OnInteraction,
}

impl HookPoint {
    /// Human-readable label for logging.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::PreTurn => "pre_turn",
            Self::PostTurn => "post_turn",
            Self::PreToolCallDecide => "pre_tool_call_decide",
            Self::PostToolCall => "post_tool_call",
            Self::OnCompaction => "on_compaction",
            Self::OnSessionStart => "on_session_start",
            Self::OnSessionEnd => "on_session_end",
            Self::OnToolError => "on_tool_error",
            Self::OnInteraction => "on_interaction",
        }
    }
}

/// A named hook registration that will be attached to an agent.
///
/// The `callback_id` is an opaque identifier used to look up the actual
/// Rust callback in the hook runner. This decouples serialization from
/// function pointers.
///
/// # Construction
///
/// Prefer [`HookEntry::new`] which validates eagerly. Direct struct
/// construction is allowed for deserialization but skips validation —
/// call [`HookEntry::validate`] before use if constructing manually.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookEntry {
    /// Descriptive name (e.g. `"safety_gate"`).
    pub name: String,
    /// Which lifecycle point this hook fires at.
    pub point: HookPoint,
    /// Opaque callback identifier for the hook runner to resolve.
    pub callback_id: String,
}

impl HookEntry {
    /// Create a new hook entry, validating that `name` and `callback_id`
    /// are non-empty.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidConfig`](crate::error::Error::InvalidConfig)
    /// if `name` or `callback_id` is empty or whitespace-only.
    ///
    /// # Examples
    ///
    /// ```
    /// # use agy_bridge::hooks::{HookEntry, HookPoint};
    /// let entry = HookEntry::new("safety_gate", HookPoint::PreToolCallDecide, "cb_safety")
    ///     .expect("valid entry");
    /// assert_eq!(entry.name, "safety_gate");
    /// ```
    pub fn new(
        name: impl Into<String>,
        point: HookPoint,
        callback_id: impl Into<String>,
    ) -> Result<Self, crate::error::Error> {
        let entry = Self {
            name: name.into(),
            point,
            callback_id: callback_id.into(),
        };
        entry.validate()?;
        Ok(entry)
    }

    /// Validate that the entry has non-empty name and `callback_id`.
    ///
    /// # Errors
    ///
    /// Returns `Err` with a description if the name or `callback_id` is empty.
    pub fn validate(&self) -> Result<(), crate::error::Error> {
        if self.name.trim().is_empty() {
            return Err(crate::error::Error::InvalidConfig {
                message: "HookEntry name must not be empty".to_owned(),
            });
        }
        if self.callback_id.trim().is_empty() {
            return Err(crate::error::Error::InvalidConfig {
                message: format!("HookEntry '{}' has an empty callback_id", self.name),
            });
        }
        Ok(())
    }
}

/// An ordered list of hooks to attach to an agent.
///
/// Hooks at the same [`HookPoint`] fire in registration order.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HookSet {
    entries: Vec<HookEntry>,
}

impl HookSet {
    /// Create an empty hook set.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Register a hook.
    ///
    /// If a hook with the same name AND hook point already exists, it is
    /// replaced and a warning is logged.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the entry fails validation (empty name or `callback_id`).
    pub fn push(&mut self, entry: HookEntry) -> Result<(), crate::error::Error> {
        entry.validate()?;
        if let Some(pos) = self
            .entries
            .iter()
            .position(|e| e.name == entry.name && e.point == entry.point)
        {
            tracing::warn!(
                hook = %entry.name,
                point = %entry.point.label(),
                "duplicate hook name+point in HookSet — replacing previous entry"
            );
            self.entries[pos] = entry;
        } else {
            self.entries.push(entry);
        }
        Ok(())
    }

    /// Iterate over hooks at a specific point, in registration order.
    pub fn at_point(&self, point: HookPoint) -> impl Iterator<Item = &HookEntry> {
        self.entries.iter().filter(move |e| e.point == point)
    }

    /// Iterate over all hooks.
    pub fn iter(&self) -> impl Iterator<Item = &HookEntry> {
        self.entries.iter()
    }

    /// Number of registered hooks.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the set is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl From<HookSet> for Vec<HookEntry> {
    fn from(set: HookSet) -> Self {
        set.entries
    }
}

impl From<&HookSet> for Vec<HookEntry> {
    fn from(set: &HookSet) -> Self {
        set.entries.clone()
    }
}

impl IntoIterator for HookSet {
    type Item = HookEntry;
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.into_iter()
    }
}

impl FromIterator<HookEntry> for HookSet {
    fn from_iter<T: IntoIterator<Item = HookEntry>>(iter: T) -> Self {
        let mut set = Self::new();
        for entry in iter {
            let name = entry.name.clone();
            if let Err(e) = set.push(entry) {
                tracing::error!(
                    error = %e,
                    hook = %name,
                    "Failed to push hook entry during from_iter"
                );
            }
        }
        set
    }
}

impl From<Vec<HookEntry>> for HookSet {
    fn from(entries: Vec<HookEntry>) -> Self {
        Self::from_iter(entries)
    }
}

impl<const N: usize> From<[HookEntry; N]> for HookSet {
    fn from(entries: [HookEntry; N]) -> Self {
        Self::from_iter(entries)
    }
}
// ── Callback types ──────────────────────────────────────────────────────────

/// Type alias for the transform-tool-input closure signature.
///
/// Accepts a pre-tool-call context and optionally returns replacement
/// arguments.  `None` means "no change".
type TransformToolInputFn =
    dyn Fn(&PreToolCallDecideContext) -> Option<serde_json::Value> + Send + Sync;

/// A registered hook callback, keyed by hook point.
///
/// Each variant wraps a boxed closure that receives the strongly-typed context
/// for that hook point.  [`PreToolCallDecide`](Self::PreToolCallDecide) returns
/// a [`HookResult`] so it can approve or deny tool execution; all other
/// variants are fire-and-forget observers.
#[non_exhaustive]
pub enum HookCallback {
    /// Callback invoked before each agent turn.
    PreTurn(Box<dyn Fn(&PreTurnContext) + Send + Sync>),
    /// Callback invoked after each agent turn completes.
    PostTurn(Box<dyn Fn(&PostTurnContext) + Send + Sync>),
    /// Callback invoked before deciding whether to execute a tool call.
    PreToolCallDecide(Box<dyn Fn(&PreToolCallDecideContext) -> HookResult + Send + Sync>),
    /// Callback invoked after a tool call completes.
    PostToolCall(Box<dyn Fn(&PostToolCallContext) + Send + Sync>),
    /// Callback invoked when a tool call produces an error.
    OnToolError(Box<dyn Fn(&OnToolErrorContext) + Send + Sync>),
    /// Callback invoked when a new agent session begins.
    OnSessionStart(Box<dyn Fn(&OnSessionStartContext) + Send + Sync>),
    /// Callback invoked when an agent session ends.
    OnSessionEnd(Box<dyn Fn(&OnSessionEndContext) + Send + Sync>),
    /// Callback invoked when conversation history is compacted.
    OnCompaction(Box<dyn Fn(&OnCompactionContext) + Send + Sync>),
    /// Callback invoked on each interaction event.
    OnInteraction(Box<dyn Fn(&OnInteractionContext) -> HookResult + Send + Sync>),
    /// Transform tool input arguments before execution.
    ///
    /// The closure receives the pre-tool-call context and may return
    /// `Some(new_args)` to replace the tool arguments, or `None` to
    /// leave them unchanged.  Multiple transform hooks are applied
    /// sequentially — each receives the (possibly already-modified)
    /// arguments from the previous transform.
    TransformToolInput(Box<TransformToolInputFn>),
}

impl HookCallback {
    /// Returns the [`HookPoint`] this callback is associated with.
    #[must_use]
    pub(crate) const fn hook_point(&self) -> HookPoint {
        match self {
            Self::PreTurn(_) => HookPoint::PreTurn,
            Self::PostTurn(_) => HookPoint::PostTurn,
            Self::PreToolCallDecide(_) | Self::TransformToolInput(_) => {
                HookPoint::PreToolCallDecide
            }
            Self::PostToolCall(_) => HookPoint::PostToolCall,
            Self::OnToolError(_) => HookPoint::OnToolError,
            Self::OnSessionStart(_) => HookPoint::OnSessionStart,
            Self::OnSessionEnd(_) => HookPoint::OnSessionEnd,
            Self::OnCompaction(_) => HookPoint::OnCompaction,
            Self::OnInteraction(_) => HookPoint::OnInteraction,
        }
    }
}

// Manual Debug impl because closures don't implement Debug.
impl std::fmt::Debug for HookCallback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HookCallback::")?;
        match self {
            Self::TransformToolInput(_) => f.write_str("transform_tool_input"),
            other => f.write_str(other.hook_point().label()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_result_allow() {
        let r = HookResult::allow();
        assert!(r.allow);
        assert!(r.message.is_empty());
    }

    #[test]
    fn hook_result_deny() {
        let r = HookResult::deny("blocked by policy");
        assert!(!r.allow);
        assert_eq!(r.message, "blocked by policy");
    }

    #[test]
    fn hook_result_allow_with_message() {
        let r = HookResult::allow_with_message("proceeding with caution");
        assert!(r.allow);
        assert_eq!(r.message, "proceeding with caution");
    }

    #[test]
    fn hook_point_labels() {
        assert_eq!(HookPoint::PreTurn.label(), "pre_turn");
        assert_eq!(HookPoint::PostTurn.label(), "post_turn");
        assert_eq!(HookPoint::PreToolCallDecide.label(), "pre_tool_call_decide");
        assert_eq!(HookPoint::PostToolCall.label(), "post_tool_call");
        assert_eq!(HookPoint::OnCompaction.label(), "on_compaction");
        assert_eq!(HookPoint::OnSessionStart.label(), "on_session_start");
        assert_eq!(HookPoint::OnSessionEnd.label(), "on_session_end");
        assert_eq!(HookPoint::OnToolError.label(), "on_tool_error");
        assert_eq!(HookPoint::OnInteraction.label(), "on_interaction");
    }

    #[test]
    fn hooks_fire_in_correct_order() {
        let mut set = HookSet::new();
        assert!(set.is_empty());

        set.push(HookEntry {
            name: "pre_turn_1".to_owned(),
            point: HookPoint::PreTurn,
            callback_id: "cb_pre1".to_owned(),
        })
        .unwrap();
        set.push(HookEntry {
            name: "pre_tool_decide".to_owned(),
            point: HookPoint::PreToolCallDecide,
            callback_id: "cb_decide".to_owned(),
        })
        .unwrap();
        set.push(HookEntry {
            name: "pre_turn_2".to_owned(),
            point: HookPoint::PreTurn,
            callback_id: "cb_pre2".to_owned(),
        })
        .unwrap();
        set.push(HookEntry {
            name: "post_turn_1".to_owned(),
            point: HookPoint::PostTurn,
            callback_id: "cb_post1".to_owned(),
        })
        .unwrap();
        set.push(HookEntry {
            name: "post_tool_1".to_owned(),
            point: HookPoint::PostToolCall,
            callback_id: "cb_posttool1".to_owned(),
        })
        .unwrap();

        assert_eq!(set.len(), 5);

        let pre_turn: Vec<&str> = set
            .at_point(HookPoint::PreTurn)
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(pre_turn, vec!["pre_turn_1", "pre_turn_2"]);

        let decide: Vec<&str> = set
            .at_point(HookPoint::PreToolCallDecide)
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(decide, vec!["pre_tool_decide"]);

        let post_turn: Vec<&str> = set
            .at_point(HookPoint::PostTurn)
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(post_turn, vec!["post_turn_1"]);

        let post_tool: Vec<&str> = set
            .at_point(HookPoint::PostToolCall)
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(post_tool, vec!["post_tool_1"]);
    }

    #[test]
    fn hook_entry_serde_roundtrip() {
        let entry = HookEntry {
            name: "my_hook".to_owned(),
            point: HookPoint::PreToolCallDecide,
            callback_id: "cb_123".to_owned(),
        };
        let json = serde_json::to_string(&entry).expect("serialize");
        let parsed: HookEntry = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.name, entry.name);
        assert_eq!(parsed.point, entry.point);
        assert_eq!(parsed.callback_id, entry.callback_id);
    }

    #[test]
    fn hook_result_serde_roundtrip() {
        let results = vec![
            HookResult::allow(),
            HookResult::deny("reason"),
            HookResult::allow_with_message("ok"),
        ];
        for result in &results {
            let json = serde_json::to_string(result).expect("serialize");
            let parsed: HookResult = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(&parsed, result);
        }
    }

    #[test]
    fn hook_set_serde_roundtrip() {
        let mut set = HookSet::new();
        set.push(HookEntry {
            name: "gate".to_owned(),
            point: HookPoint::PreTurn,
            callback_id: "cb_1".to_owned(),
        })
        .unwrap();
        set.push(HookEntry {
            name: "logger".to_owned(),
            point: HookPoint::PostToolCall,
            callback_id: "cb_2".to_owned(),
        })
        .unwrap();
        let json = serde_json::to_string(&set).expect("serialize");
        let parsed: HookSet = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.len(), 2);
        let names: Vec<&str> = parsed.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["gate", "logger"]);
    }

    #[test]
    fn hook_set_from_conversions() {
        let mut set = HookSet::new();
        set.push(HookEntry {
            name: "gate".to_owned(),
            point: HookPoint::PreTurn,
            callback_id: "cb_1".to_owned(),
        })
        .unwrap();

        let vec_from_owned: Vec<HookEntry> = Vec::from(set.clone());
        assert_eq!(vec_from_owned.len(), 1);
        assert_eq!(vec_from_owned[0].name, "gate");

        let vec_from_ref: Vec<HookEntry> = Vec::from(&set);
        assert_eq!(vec_from_ref.len(), 1);
        assert_eq!(vec_from_ref[0].name, "gate");

        let entry = HookEntry {
            name: "gate".to_owned(),
            point: HookPoint::PreTurn,
            callback_id: "cb_1".to_owned(),
        };
        let set_from_arr = HookSet::from([entry.clone()]);
        assert_eq!(set_from_arr.len(), 1);

        let set_from_vec = HookSet::from(vec![entry]);
        assert_eq!(set_from_vec.len(), 1);
    }

    #[test]
    fn empty_hook_set_iteration_at_each_point() {
        let set = HookSet::new();
        for point in [
            HookPoint::PreTurn,
            HookPoint::PostTurn,
            HookPoint::PreToolCallDecide,
            HookPoint::PostToolCall,
            HookPoint::OnCompaction,
            HookPoint::OnSessionStart,
            HookPoint::OnSessionEnd,
            HookPoint::OnToolError,
            HookPoint::OnInteraction,
        ] {
            assert_eq!(
                set.at_point(point).count(),
                0,
                "Empty HookSet should have 0 hooks at {point:?}"
            );
        }
    }

    #[test]
    fn hook_point_serde_roundtrip() {
        let points = [
            HookPoint::PreTurn,
            HookPoint::PostTurn,
            HookPoint::PreToolCallDecide,
            HookPoint::PostToolCall,
            HookPoint::OnCompaction,
            HookPoint::OnSessionStart,
            HookPoint::OnSessionEnd,
            HookPoint::OnToolError,
            HookPoint::OnInteraction,
        ];
        for point in points {
            let json = serde_json::to_string(&point).expect("serialize");
            let parsed: HookPoint = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(parsed, point);
        }
    }

    #[test]
    fn hook_set_default_is_empty() {
        let set = HookSet::default();
        assert!(set.is_empty());
        assert_eq!(set.len(), 0);
    }

    #[test]
    fn hook_set_multiple_hooks_at_same_point() {
        let mut set = HookSet::new();
        for i in 0..5 {
            set.push(HookEntry {
                name: format!("hook_{i}"),
                point: HookPoint::PreToolCallDecide,
                callback_id: format!("cb_{i}"),
            })
            .unwrap();
        }
        assert_eq!(set.len(), 5);
        assert_eq!(set.at_point(HookPoint::PreToolCallDecide).count(), 5);
        assert_eq!(set.at_point(HookPoint::PreTurn).count(), 0);
    }

    #[test]
    fn hook_result_deny_with_string_owned() {
        let reason = String::from("policy violation detected");
        let r = HookResult::deny(reason.clone());
        assert!(!r.allow);
        assert_eq!(r.message, reason);
    }

    #[test]
    fn hook_entry_with_new_hook_points() {
        let new_points = [
            (HookPoint::OnCompaction, "compaction_hook"),
            (HookPoint::OnSessionStart, "session_start_hook"),
            (HookPoint::OnSessionEnd, "session_end_hook"),
            (HookPoint::OnToolError, "tool_error_hook"),
            (HookPoint::OnInteraction, "interaction_hook"),
        ];
        let mut set = HookSet::new();
        for (point, name) in &new_points {
            set.push(HookEntry {
                name: (*name).to_owned(),
                point: *point,
                callback_id: format!("cb_{name}"),
            })
            .unwrap();
        }
        assert_eq!(set.len(), 5);
        for (point, name) in &new_points {
            let hooks: Vec<&str> = set.at_point(*point).map(|e| e.name.as_str()).collect();
            assert_eq!(hooks, vec![*name], "expected hook at {point:?}");
        }
    }

    #[test]
    fn hook_entry_serde_roundtrip_new_points() {
        let new_points = [
            HookPoint::OnCompaction,
            HookPoint::OnSessionStart,
            HookPoint::OnSessionEnd,
            HookPoint::OnToolError,
            HookPoint::OnInteraction,
        ];
        for point in new_points {
            let entry = HookEntry {
                name: format!("test_{}", point.label()),
                point,
                callback_id: format!("cb_{}", point.label()),
            };
            let json = serde_json::to_string(&entry).expect("serialize");
            let parsed: HookEntry = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(parsed.name, entry.name);
            assert_eq!(parsed.point, entry.point);
            assert_eq!(parsed.callback_id, entry.callback_id);
        }
    }

    // ── SessionContext tests ────────────────────────────────────────────

    #[test]
    fn session_context_clone() {
        let ctx = SessionContext {
            session_id: "sess-1".into(),
            agent_id: 42,
            started_at: Instant::now(),
        };
        let cloned = ctx;
        assert_eq!(cloned.session_id, "sess-1");
        assert_eq!(cloned.agent_id, 42);
    }

    #[test]
    fn session_context_debug_format() {
        let ctx = SessionContext {
            session_id: "sess-debug".into(),
            agent_id: 1,
            started_at: Instant::now(),
        };
        let dbg = format!("{ctx:?}");
        assert!(dbg.contains("sess-debug"));
        assert!(dbg.contains("agent_id: 1"));
    }

    // ── HookEntry::new validated constructor tests ──────────────────────

    #[test]
    fn hook_entry_new_valid() {
        let entry = HookEntry::new("safety_gate", HookPoint::PreToolCallDecide, "cb_safety")
            .expect("valid entry");
        assert_eq!(entry.name, "safety_gate");
        assert_eq!(entry.point, HookPoint::PreToolCallDecide);
        assert_eq!(entry.callback_id, "cb_safety");
    }

    #[test]
    fn hook_entry_new_rejects_empty_name() {
        let result = HookEntry::new("", HookPoint::PreTurn, "cb_1");
        assert!(result.is_err(), "should reject empty name");
    }

    #[test]
    fn hook_entry_new_rejects_whitespace_name() {
        let result = HookEntry::new("   ", HookPoint::PreTurn, "cb_1");
        assert!(result.is_err(), "should reject whitespace-only name");
    }

    #[test]
    fn hook_entry_new_rejects_empty_callback_id() {
        let result = HookEntry::new("my_hook", HookPoint::PreTurn, "");
        assert!(result.is_err(), "should reject empty callback_id");
    }

    #[test]
    fn hook_entry_new_rejects_whitespace_callback_id() {
        let result = HookEntry::new("my_hook", HookPoint::PostTurn, "  ");
        assert!(result.is_err(), "should reject whitespace-only callback_id");
    }

    #[test]
    fn pre_tool_call_decide_context_serde_aliases() {
        let json_std = r#"{"tool_name":"my_tool","tool_args":{"foo":"bar"}}"#;
        let parsed_std: PreToolCallDecideContext = serde_json::from_str(json_std).unwrap();
        assert_eq!(parsed_std.tool_name, "my_tool");
        assert_eq!(parsed_std.tool_args["foo"], "bar");

        let json_alias = r#"{"name":"my_tool","args":{"foo":"bar"}}"#;
        let parsed_alias: PreToolCallDecideContext = serde_json::from_str(json_alias).unwrap();
        assert_eq!(parsed_alias.tool_name, "my_tool");
        assert_eq!(parsed_alias.tool_args["foo"], "bar");
    }

    #[test]
    fn pre_tool_call_decide_context_serde_default() {
        let json_no_args = r#"{"name":"my_tool"}"#;
        let parsed_no_args: PreToolCallDecideContext = serde_json::from_str(json_no_args).unwrap();
        assert_eq!(parsed_no_args.tool_name, "my_tool");
        assert_eq!(parsed_no_args.tool_args, serde_json::Value::Null);
    }

    #[test]
    fn post_tool_call_context_serde_aliases_and_default() {
        let json_std = r#"{"tool_name":"my_tool","tool_args":{"foo":"bar"},"result":"success"}"#;
        let parsed_std: PostToolCallContext = serde_json::from_str(json_std).unwrap();
        assert_eq!(parsed_std.tool_name, "my_tool");
        assert_eq!(parsed_std.tool_args["foo"], "bar");
        assert_eq!(parsed_std.result, "success");

        let json_alias = r#"{"name":"my_tool","args":{"foo":"bar"},"result":"success"}"#;
        let parsed_alias: PostToolCallContext = serde_json::from_str(json_alias).unwrap();
        assert_eq!(parsed_alias.tool_name, "my_tool");
        assert_eq!(parsed_alias.tool_args["foo"], "bar");
        assert_eq!(parsed_alias.result, "success");

        let json_no_args = r#"{"name":"my_tool","result":"success"}"#;
        let parsed_no_args: PostToolCallContext = serde_json::from_str(json_no_args).unwrap();
        assert_eq!(parsed_no_args.tool_name, "my_tool");
        assert_eq!(parsed_no_args.tool_args, serde_json::Value::Null);
        assert_eq!(parsed_no_args.result, "success");
    }

    #[test]
    fn on_tool_error_context_serde_aliases_and_default() {
        let json_std = r#"{"tool_name":"my_tool","tool_args":{"foo":"bar"},"error":"failed"}"#;
        let parsed_std: OnToolErrorContext = serde_json::from_str(json_std).unwrap();
        assert_eq!(parsed_std.tool_name, "my_tool");
        assert_eq!(parsed_std.tool_args["foo"], "bar");
        assert_eq!(parsed_std.error, "failed");

        let json_alias = r#"{"name":"my_tool","args":{"foo":"bar"},"error":"failed"}"#;
        let parsed_alias: OnToolErrorContext = serde_json::from_str(json_alias).unwrap();
        assert_eq!(parsed_alias.tool_name, "my_tool");
        assert_eq!(parsed_alias.tool_args["foo"], "bar");
        assert_eq!(parsed_alias.error, "failed");

        let json_no_args = r#"{"name":"my_tool","error":"failed"}"#;
        let parsed_no_args: OnToolErrorContext = serde_json::from_str(json_no_args).unwrap();
        assert_eq!(parsed_no_args.tool_name, "my_tool");
        assert_eq!(parsed_no_args.tool_args, serde_json::Value::Null);
        assert_eq!(parsed_no_args.error, "failed");

        let json_no_name = r#"{"error":"failed"}"#;
        let parsed_no_name: Result<OnToolErrorContext, _> = serde_json::from_str(json_no_name);
        assert!(parsed_no_name.is_err());
    }
}

//! Hook runner: registration and execution of lifecycle callbacks.

use super::types::{
    HookCallback, HookPoint, HookResult, OnCompactionContext, OnInteractionContext,
    OnSessionEndContext, OnSessionStartContext, OnToolErrorContext, PostToolCallContext,
    PostTurnContext, PreToolCallDecideContext, PreTurnContext,
};

// ── Hook runner ─────────────────────────────────────────────────────────────

/// Stores and executes registered hook callbacks.
///
/// Callbacks at the same [`HookPoint`] fire in the order they were registered.
///
/// # Example
///
/// Fluent builder pattern (recommended):
///
/// ```
/// use agy_bridge::hooks::{HookResult, Hooks, PreToolCallDecideContext, PreTurnContext};
///
/// let hooks = Hooks::new()
///     .with_pre_turn("logger", |ctx: &PreTurnContext| {
///         println!("Turn {} prompt: {}", ctx.turn_number, ctx.prompt);
///     })
///     .with_pre_tool_call_decide("gate", |ctx: &PreToolCallDecideContext| {
///         if ctx.tool_name == "dangerous_tool" {
///             HookResult::deny("blocked by policy")
///         } else {
///             HookResult::allow()
///         }
///     });
///
/// hooks.run_pre_turn(&PreTurnContext {
///     prompt: "hi".into(),
///     turn_number: 1,
/// });
/// let result = hooks.run_pre_tool_call_decide(&PreToolCallDecideContext {
///     tool_name: "safe_tool".into(),
///     tool_args: serde_json::Value::Null,
/// });
/// assert!(result.allow);
/// ```
///
/// For conditional or loop-based registration, use the `on_*(&mut self)` methods:
///
/// ```
/// # use agy_bridge::hooks::{HookResult, Hooks};
/// let mut hooks = Hooks::new();
/// hooks.on_pre_turn("logger", |ctx| {
///     println!("Turn {}", ctx.turn_number);
/// });
/// ```
pub struct Hooks {
    callbacks: Vec<(HookPoint, String, HookCallback)>,
}

impl Hooks {
    /// Create an empty hook runner.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            callbacks: Vec::new(),
        }
    }

    /// Register a named callback.
    ///
    /// The [`HookPoint`] is derived automatically from the callback variant.
    /// If a callback with the same name AND hook point already exists, it is
    /// replaced and a warning is logged.
    /// Returns `&mut Self` for chaining.
    pub fn register(&mut self, name: impl Into<String>, callback: HookCallback) -> &mut Self {
        let point = callback.hook_point();
        let name = name.into();
        if let Some(pos) = self
            .callbacks
            .iter()
            .position(|(p, n, _)| *p == point && n == &name)
        {
            tracing::warn!(
                hook = %name,
                point = %point.label(),
                "duplicate hook name+point in Hooks — replacing previous callback"
            );
            self.callbacks[pos] = (point, name, callback);
        } else {
            tracing::debug!(hook = %name, point = %point.label(), "registered hook callback");
            self.callbacks.push((point, name, callback));
        }
        self
    }

    /// Run all [`HookPoint::PreTurn`] callbacks in registration order.
    pub fn run_pre_turn(&self, ctx: &PreTurnContext) {
        for (_, name, cb) in self.iter_at(HookPoint::PreTurn) {
            tracing::trace!(hook = %name, turn = ctx.turn_number, "firing pre_turn hook");
            if let HookCallback::PreTurn(f) = cb {
                let name = name.clone();
                if let Err(panic) =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(ctx)))
                {
                    tracing::error!(hook = %name, panic = ?panic, "pre_turn hook panicked — continuing");
                }
            }
        }
    }

    /// Run all [`HookPoint::PostTurn`] callbacks in registration order.
    pub fn run_post_turn(&self, ctx: &PostTurnContext) {
        for (_, name, cb) in self.iter_at(HookPoint::PostTurn) {
            tracing::trace!(hook = %name, turn = ctx.turn_number, "firing post_turn hook");
            if let HookCallback::PostTurn(f) = cb {
                let name = name.clone();
                if let Err(panic) =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(ctx)))
                {
                    tracing::error!(hook = %name, panic = ?panic, "post_turn hook panicked — continuing");
                }
            }
        }
    }

    /// Run all [`HookPoint::PreToolCallDecide`] callbacks in registration order.
    ///
    /// If any callback returns [`HookResult`] with `allow: false`, execution
    /// short-circuits and that deny result is returned immediately.  Otherwise
    /// returns [`HookResult::allow()`].
    ///
    /// If a callback panics, the tool call is denied as a safe default.
    pub fn run_pre_tool_call_decide(&self, ctx: &PreToolCallDecideContext) -> HookResult {
        for (_, name, cb) in self.iter_at(HookPoint::PreToolCallDecide) {
            tracing::trace!(hook = %name, tool = %ctx.tool_name, "firing pre_tool_call_decide hook");
            if let HookCallback::PreToolCallDecide(f) = cb {
                let result = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(ctx)))
                {
                    Ok(r) => r,
                    Err(panic) => {
                        tracing::error!(
                            hook = %name,
                            tool = %ctx.tool_name,
                            panic = ?panic,
                            "pre_tool_call_decide hook panicked — denying tool call as safe default"
                        );
                        return HookResult::deny(format!(
                            "hook '{name}' panicked — tool call denied as safe default"
                        ));
                    }
                };
                if !result.allow {
                    tracing::info!(
                        hook = %name,
                        tool = %ctx.tool_name,
                        reason = %result.message,
                        "tool call denied by hook"
                    );
                    return result;
                }
            }
        }
        HookResult::allow()
    }

    /// Run all [`HookPoint::PostToolCall`] callbacks in registration order.
    pub fn run_post_tool_call(&self, ctx: &PostToolCallContext) {
        for (_, name, cb) in self.iter_at(HookPoint::PostToolCall) {
            tracing::trace!(hook = %name, tool = %ctx.tool_name, "firing post_tool_call hook");
            if let HookCallback::PostToolCall(f) = cb {
                let name = name.clone();
                if let Err(panic) =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(ctx)))
                {
                    tracing::error!(hook = %name, panic = ?panic, "post_tool_call hook panicked — continuing");
                }
            }
        }
    }

    /// Run all [`HookPoint::OnToolError`] callbacks in registration order.
    pub fn run_on_tool_error(&self, ctx: &OnToolErrorContext) {
        for (_, name, cb) in self.iter_at(HookPoint::OnToolError) {
            tracing::trace!(hook = %name, tool = %ctx.tool_name, error = %ctx.error, "firing on_tool_error hook");
            if let HookCallback::OnToolError(f) = cb {
                let name = name.clone();
                if let Err(panic) =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(ctx)))
                {
                    tracing::error!(hook = %name, panic = ?panic, "on_tool_error hook panicked — continuing");
                }
            }
        }
    }

    /// Run all [`HookPoint::OnSessionStart`] callbacks in registration order.
    pub fn run_on_session_start(&self, ctx: &OnSessionStartContext) {
        for (_, name, cb) in self.iter_at(HookPoint::OnSessionStart) {
            tracing::trace!(hook = %name, "firing on_session_start hook");
            if let HookCallback::OnSessionStart(f) = cb {
                let name = name.clone();
                if let Err(panic) =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(ctx)))
                {
                    tracing::error!(hook = %name, panic = ?panic, "on_session_start hook panicked — continuing");
                }
            }
        }
    }

    /// Run all [`HookPoint::OnSessionEnd`] callbacks in registration order.
    pub fn run_on_session_end(&self, ctx: &OnSessionEndContext) {
        for (_, name, cb) in self.iter_at(HookPoint::OnSessionEnd) {
            tracing::trace!(hook = %name, "firing on_session_end hook");
            if let HookCallback::OnSessionEnd(f) = cb {
                let name = name.clone();
                if let Err(panic) =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(ctx)))
                {
                    tracing::error!(hook = %name, panic = ?panic, "on_session_end hook panicked — continuing");
                }
            }
        }
    }

    /// Run all [`HookPoint::OnCompaction`] callbacks in registration order.
    pub fn run_on_compaction(&self, ctx: &OnCompactionContext) {
        for (_, name, cb) in self.iter_at(HookPoint::OnCompaction) {
            tracing::trace!(hook = %name, "firing on_compaction hook");
            if let HookCallback::OnCompaction(f) = cb {
                let name = name.clone();
                if let Err(panic) =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(ctx)))
                {
                    tracing::error!(hook = %name, panic = ?panic, "on_compaction hook panicked — continuing");
                }
            }
        }
    }

    /// Run all [`HookPoint::OnInteraction`] callbacks in registration order.
    ///
    /// If a callback panics, the panic is logged and execution continues
    /// (the interaction is not blocked).
    pub fn run_on_interaction(&self, ctx: &OnInteractionContext) -> HookResult {
        for (_, name, cb) in self.iter_at(HookPoint::OnInteraction) {
            tracing::trace!(hook = %name, "firing on_interaction hook");
            if let HookCallback::OnInteraction(f) = cb {
                let result = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(ctx)))
                {
                    Ok(r) => r,
                    Err(panic) => {
                        tracing::error!(
                            hook = %name,
                            panic = ?panic,
                            "on_interaction hook panicked — continuing"
                        );
                        continue;
                    }
                };
                if !result.allow {
                    return result;
                }
            }
        }
        HookResult::allow()
    }

    /// Run all [`TransformToolInput`](HookCallback::TransformToolInput)
    /// callbacks in registration order, threading the (possibly modified)
    /// tool arguments through each transform.
    ///
    /// Returns the final tool arguments after all transforms have been
    /// applied.  If no transform returns `Some`, the original arguments
    /// are returned unchanged.
    ///
    /// Panicking transforms are logged and skipped (original args kept).
    pub fn run_transform_tool_input(&self, ctx: &PreToolCallDecideContext) -> serde_json::Value {
        let mut args = ctx.tool_args.clone();
        for (_, name, cb) in self.iter_at(HookPoint::PreToolCallDecide) {
            if let HookCallback::TransformToolInput(f) = cb {
                let current_ctx = PreToolCallDecideContext {
                    tool_name: ctx.tool_name.clone(),
                    tool_args: args.clone(),
                };
                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&current_ctx))) {
                    Ok(Some(new_args)) => {
                        tracing::debug!(
                            hook = %name,
                            tool = %ctx.tool_name,
                            "transform_tool_input hook modified tool arguments"
                        );
                        args = new_args;
                    }
                    Ok(None) => { /* no modification */ }
                    Err(panic) => {
                        tracing::error!(
                            hook = %name,
                            tool = %ctx.tool_name,
                            panic = ?panic,
                            "transform_tool_input hook panicked — keeping current args"
                        );
                    }
                }
            }
        }
        args
    }

    // ── Convenience builder methods (Python decorator parity) ────────

    /// Register a [`HookPoint::PreTurn`] callback.
    ///
    /// Convenience wrapper matching the Python SDK's `@on_pre_turn` decorator.
    pub fn on_pre_turn(
        &mut self,
        name: impl Into<String>,
        f: impl Fn(&PreTurnContext) + Send + Sync + 'static,
    ) -> &mut Self {
        self.register(name, HookCallback::PreTurn(Box::new(f)))
    }

    /// Register a [`HookPoint::PostTurn`] callback.
    ///
    /// Convenience wrapper matching the Python SDK's `@on_post_turn` decorator.
    pub fn on_post_turn(
        &mut self,
        name: impl Into<String>,
        f: impl Fn(&PostTurnContext) + Send + Sync + 'static,
    ) -> &mut Self {
        self.register(name, HookCallback::PostTurn(Box::new(f)))
    }

    /// Register a [`HookPoint::PreToolCallDecide`] callback.
    ///
    /// Convenience wrapper matching the Python SDK's `@on_pre_tool_call_decide`
    /// decorator.
    pub fn on_pre_tool_call_decide(
        &mut self,
        name: impl Into<String>,
        f: impl Fn(&PreToolCallDecideContext) -> HookResult + Send + Sync + 'static,
    ) -> &mut Self {
        self.register(name, HookCallback::PreToolCallDecide(Box::new(f)))
    }

    /// Register a [`HookPoint::PostToolCall`] callback.
    ///
    /// Convenience wrapper matching the Python SDK's `@on_post_tool_call` decorator.
    pub fn on_post_tool_call(
        &mut self,
        name: impl Into<String>,
        f: impl Fn(&PostToolCallContext) + Send + Sync + 'static,
    ) -> &mut Self {
        self.register(name, HookCallback::PostToolCall(Box::new(f)))
    }

    /// Register a [`HookPoint::OnToolError`] callback.
    ///
    /// Convenience wrapper matching the Python SDK's `@on_tool_error` decorator.
    pub fn on_tool_error(
        &mut self,
        name: impl Into<String>,
        f: impl Fn(&OnToolErrorContext) + Send + Sync + 'static,
    ) -> &mut Self {
        self.register(name, HookCallback::OnToolError(Box::new(f)))
    }

    /// Register a [`HookPoint::OnCompaction`] callback.
    ///
    /// Convenience wrapper matching the Python SDK's `@on_compaction` decorator.
    pub fn on_compaction(
        &mut self,
        name: impl Into<String>,
        f: impl Fn(&OnCompactionContext) + Send + Sync + 'static,
    ) -> &mut Self {
        self.register(name, HookCallback::OnCompaction(Box::new(f)))
    }

    /// Register a [`HookPoint::OnInteraction`] callback.
    ///
    /// Convenience wrapper matching the Python SDK's `@on_interaction` decorator.
    pub fn on_interaction(
        &mut self,
        name: impl Into<String>,
        f: impl Fn(&OnInteractionContext) -> HookResult + Send + Sync + 'static,
    ) -> &mut Self {
        self.register(name, HookCallback::OnInteraction(Box::new(f)))
    }

    /// Register a [`HookPoint::OnSessionStart`] callback.
    ///
    /// Convenience wrapper matching the Python SDK's `@on_session_start` decorator.
    pub fn on_session_start(
        &mut self,
        name: impl Into<String>,
        f: impl Fn(&OnSessionStartContext) + Send + Sync + 'static,
    ) -> &mut Self {
        self.register(name, HookCallback::OnSessionStart(Box::new(f)))
    }

    /// Register a [`HookPoint::OnSessionEnd`] callback.
    ///
    /// Convenience wrapper matching the Python SDK's `@on_session_end` decorator.
    pub fn on_session_end(
        &mut self,
        name: impl Into<String>,
        f: impl Fn(&OnSessionEndContext) + Send + Sync + 'static,
    ) -> &mut Self {
        self.register(name, HookCallback::OnSessionEnd(Box::new(f)))
    }

    /// Register a [`TransformToolInput`](HookCallback::TransformToolInput) callback.
    ///
    /// The closure receives the pre-tool-call context and may return
    /// `Some(new_args)` to replace tool arguments, or `None` to leave them
    /// unchanged.
    pub fn on_transform_tool_input(
        &mut self,
        name: impl Into<String>,
        f: impl Fn(&PreToolCallDecideContext) -> Option<serde_json::Value> + Send + Sync + 'static,
    ) -> &mut Self {
        self.register(name, HookCallback::TransformToolInput(Box::new(f)))
    }

    // ── Owned-self builder methods (for fluent chaining) ────────────

    /// Register a [`HookPoint::PreTurn`] callback, returning `self` for chaining.
    ///
    /// This is the owned-self variant of [`on_pre_turn`](Self::on_pre_turn).
    #[must_use]
    pub fn with_pre_turn(
        mut self,
        name: impl Into<String>,
        f: impl Fn(&PreTurnContext) + Send + Sync + 'static,
    ) -> Self {
        self.on_pre_turn(name, f);
        self
    }

    /// Register a [`HookPoint::PostTurn`] callback, returning `self` for chaining.
    ///
    /// This is the owned-self variant of [`on_post_turn`](Self::on_post_turn).
    #[must_use]
    pub fn with_post_turn(
        mut self,
        name: impl Into<String>,
        f: impl Fn(&PostTurnContext) + Send + Sync + 'static,
    ) -> Self {
        self.on_post_turn(name, f);
        self
    }

    /// Register a [`HookPoint::PreToolCallDecide`] callback, returning `self`
    /// for chaining.
    ///
    /// This is the owned-self variant of
    /// [`on_pre_tool_call_decide`](Self::on_pre_tool_call_decide).
    #[must_use]
    pub fn with_pre_tool_call_decide(
        mut self,
        name: impl Into<String>,
        f: impl Fn(&PreToolCallDecideContext) -> HookResult + Send + Sync + 'static,
    ) -> Self {
        self.on_pre_tool_call_decide(name, f);
        self
    }

    /// Register a [`HookPoint::PostToolCall`] callback, returning `self` for
    /// chaining.
    ///
    /// This is the owned-self variant of
    /// [`on_post_tool_call`](Self::on_post_tool_call).
    #[must_use]
    pub fn with_post_tool_call(
        mut self,
        name: impl Into<String>,
        f: impl Fn(&PostToolCallContext) + Send + Sync + 'static,
    ) -> Self {
        self.on_post_tool_call(name, f);
        self
    }

    /// Register a [`HookPoint::OnToolError`] callback, returning `self` for
    /// chaining.
    ///
    /// This is the owned-self variant of
    /// [`on_tool_error`](Self::on_tool_error).
    #[must_use]
    pub fn with_tool_error(
        mut self,
        name: impl Into<String>,
        f: impl Fn(&OnToolErrorContext) + Send + Sync + 'static,
    ) -> Self {
        self.on_tool_error(name, f);
        self
    }

    /// Register a [`HookPoint::OnCompaction`] callback, returning `self` for
    /// chaining.
    ///
    /// This is the owned-self variant of
    /// [`on_compaction`](Self::on_compaction).
    #[must_use]
    pub fn with_compaction(
        mut self,
        name: impl Into<String>,
        f: impl Fn(&OnCompactionContext) + Send + Sync + 'static,
    ) -> Self {
        self.on_compaction(name, f);
        self
    }

    /// Register a [`HookPoint::OnInteraction`] callback, returning `self` for
    /// chaining.
    ///
    /// This is the owned-self variant of
    /// [`on_interaction`](Self::on_interaction).
    #[must_use]
    pub fn with_interaction(
        mut self,
        name: impl Into<String>,
        f: impl Fn(&OnInteractionContext) -> HookResult + Send + Sync + 'static,
    ) -> Self {
        self.on_interaction(name, f);
        self
    }

    /// Register a [`HookPoint::OnSessionStart`] callback, returning `self`
    /// for chaining.
    ///
    /// This is the owned-self variant of
    /// [`on_session_start`](Self::on_session_start).
    #[must_use]
    pub fn with_session_start(
        mut self,
        name: impl Into<String>,
        f: impl Fn(&OnSessionStartContext) + Send + Sync + 'static,
    ) -> Self {
        self.on_session_start(name, f);
        self
    }

    /// Register a [`HookPoint::OnSessionEnd`] callback, returning `self` for
    /// chaining.
    ///
    /// This is the owned-self variant of
    /// [`on_session_end`](Self::on_session_end).
    #[must_use]
    pub fn with_session_end(
        mut self,
        name: impl Into<String>,
        f: impl Fn(&OnSessionEndContext) + Send + Sync + 'static,
    ) -> Self {
        self.on_session_end(name, f);
        self
    }

    /// Register a [`TransformToolInput`](HookCallback::TransformToolInput)
    /// callback, returning `self` for chaining.
    ///
    /// This is the owned-self variant of
    /// [`on_transform_tool_input`](Self::on_transform_tool_input).
    #[must_use]
    pub fn with_transform_tool_input(
        mut self,
        name: impl Into<String>,
        f: impl Fn(&PreToolCallDecideContext) -> Option<serde_json::Value> + Send + Sync + 'static,
    ) -> Self {
        self.on_transform_tool_input(name, f);
        self
    }

    /// Iterate callbacks at a given hook point in registration order.
    fn iter_at(
        &self,
        point: HookPoint,
    ) -> impl Iterator<Item = &(HookPoint, String, HookCallback)> {
        self.callbacks.iter().filter(move |(p, _, _)| *p == point)
    }

    /// Extract a list of [`HookEntry`](super::types::HookEntry) objects
    /// corresponding to the registered callbacks.
    ///
    /// This allows the `AgentBuilder` to automatically populate the agent's
    /// configuration with the necessary entries to connect the Python SDK's
    /// hook dispatcher back to the Rust runner.
    #[must_use]
    pub fn entries(&self) -> Vec<super::types::HookEntry> {
        self.callbacks
            .iter()
            .map(|(point, name, _)| super::types::HookEntry {
                name: name.clone(),
                point: *point,
                callback_id: name.clone(),
            })
            .collect()
    }
}

impl Default for Hooks {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::{super::types::SessionContext, *};

    #[test]
    fn hook_runner_pre_turn_callback_fires() {
        use std::sync::{
            Arc,
            atomic::{AtomicU32, Ordering},
        };

        let counter = Arc::new(AtomicU32::new(0));
        let c = Arc::clone(&counter);

        let mut runner = Hooks::new();
        runner.register(
            "counter",
            HookCallback::PreTurn(Box::new(move |ctx| {
                c.fetch_add(ctx.turn_number, Ordering::SeqCst);
            })),
        );

        runner.run_pre_turn(&PreTurnContext {
            prompt: "hello".into(),
            turn_number: 7,
        });
        assert_eq!(counter.load(Ordering::SeqCst), 7);
    }

    #[test]
    fn hook_runner_pre_tool_call_decide_deny_short_circuits() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };

        let mut runner = Hooks::new();

        // First callback: allow
        runner.register(
            "allow_gate",
            HookCallback::PreToolCallDecide(Box::new(|_ctx| HookResult::allow())),
        );

        // Second callback: deny
        runner.register(
            "deny_gate",
            HookCallback::PreToolCallDecide(Box::new(|_ctx| HookResult::deny("blocked"))),
        );

        // Third callback: should never be reached
        let reached = Arc::new(AtomicBool::new(false));
        let r = Arc::clone(&reached);
        runner.register(
            "unreachable",
            HookCallback::PreToolCallDecide(Box::new(move |_ctx| {
                r.store(true, Ordering::SeqCst);
                HookResult::allow()
            })),
        );

        let result = runner.run_pre_tool_call_decide(&PreToolCallDecideContext {
            tool_name: "some_tool".into(),
            tool_args: serde_json::Value::Null,
        });

        assert!(!result.allow);
        assert_eq!(result.message, "blocked");
        assert!(
            !reached.load(Ordering::SeqCst),
            "third callback should not fire after deny"
        );
    }

    #[test]
    fn hook_runner_multiple_callbacks_fire_in_order() {
        use std::sync::{Arc, Mutex};

        let log = Arc::new(Mutex::new(Vec::<String>::new()));

        let mut runner = Hooks::new();
        for i in 0..3 {
            let l = Arc::clone(&log);
            runner.register(
                format!("hook_{i}"),
                HookCallback::PreTurn(Box::new(move |_ctx| {
                    l.lock().unwrap().push(format!("hook_{i}"));
                })),
            );
        }

        runner.run_pre_turn(&PreTurnContext {
            prompt: "test".into(),
            turn_number: 1,
        });

        let entries = log.lock().unwrap().clone();
        assert_eq!(entries, vec!["hook_0", "hook_1", "hook_2"]);
    }

    #[test]
    fn hook_runner_post_tool_call_receives_result() {
        use std::sync::{Arc, Mutex};

        let captured = Arc::new(Mutex::new(String::new()));
        let c = Arc::clone(&captured);

        let mut runner = Hooks::new();
        runner.register(
            "capture",
            HookCallback::PostToolCall(Box::new(move |ctx| {
                *c.lock().unwrap() = ctx.result.clone();
            })),
        );

        runner.run_post_tool_call(&PostToolCallContext {
            tool_name: "read_file".into(),
            tool_args: serde_json::json!({"path": "/tmp/x"}),
            result: "file contents here".into(),
        });

        assert_eq!(*captured.lock().unwrap(), "file contents here");
    }

    #[test]
    fn hook_runner_on_tool_error_fires_with_context() {
        use std::sync::{Arc, Mutex};

        let captured_error = Arc::new(Mutex::new(String::new()));
        let captured_tool = Arc::new(Mutex::new(String::new()));
        let ce = Arc::clone(&captured_error);
        let ct = Arc::clone(&captured_tool);

        let mut runner = Hooks::new();
        runner.register(
            "error_logger",
            HookCallback::OnToolError(Box::new(move |ctx| {
                *ce.lock().unwrap() = ctx.error.clone();
                *ct.lock().unwrap() = ctx.tool_name.clone();
            })),
        );

        runner.run_on_tool_error(&OnToolErrorContext {
            tool_name: "write_file".into(),
            tool_args: serde_json::json!({}),
            error: "permission denied".into(),
        });

        assert_eq!(*captured_error.lock().unwrap(), "permission denied");
        assert_eq!(*captured_tool.lock().unwrap(), "write_file");
    }

    #[test]
    fn hook_runner_default_is_empty() {
        let runner = Hooks::default();
        runner.run_pre_turn(&PreTurnContext {
            prompt: "x".into(),
            turn_number: 1,
        });
        let result = runner.run_pre_tool_call_decide(&PreToolCallDecideContext {
            tool_name: "t".into(),
            tool_args: serde_json::Value::Null,
        });
        assert!(result.allow, "empty runner should allow everything");
    }

    #[test]
    fn hook_callback_debug_format() {
        let cb = HookCallback::PreTurn(Box::new(|_| {}));
        let dbg = format!("{cb:?}");
        assert_eq!(dbg, "HookCallback::pre_turn");
    }

    // ── Panic recovery tests ────────────────────────────────────────────

    #[test]
    fn hook_runner_pre_turn_panic_recovery() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };

        let reached = Arc::new(AtomicBool::new(false));
        let r = Arc::clone(&reached);

        let mut runner = Hooks::new();
        // Register a hook that panics.
        runner.register(
            "panicker",
            HookCallback::PreTurn(Box::new(|_ctx| {
                panic!("intentional test panic in pre_turn hook");
            })),
        );
        // Register a second hook after the panicking one — it should still fire.
        runner.register(
            "after_panic",
            HookCallback::PreTurn(Box::new(move |_ctx| {
                r.store(true, Ordering::SeqCst);
            })),
        );

        // run_pre_turn should NOT propagate the panic.
        runner.run_pre_turn(&PreTurnContext {
            prompt: "test".into(),
            turn_number: 1,
        });

        assert!(
            reached.load(Ordering::SeqCst),
            "second hook should fire even after the first panicked"
        );
    }

    #[test]
    fn hook_runner_pre_tool_call_decide_panic_returns_deny() {
        let mut runner = Hooks::new();
        // Register a hook that panics.
        runner.register(
            "panicker",
            HookCallback::PreToolCallDecide(Box::new(|_ctx| {
                panic!("intentional test panic in pre_tool_call_decide");
            })),
        );

        let result = runner.run_pre_tool_call_decide(&PreToolCallDecideContext {
            tool_name: "dangerous_tool".into(),
            tool_args: serde_json::Value::Null,
        });

        // A panicking PreToolCallDecide hook should deny the tool call as a safe default.
        assert!(!result.allow, "panicking hook should deny the tool call");
        assert!(
            result.message.contains("panicked"),
            "deny message should mention the panic: {:?}",
            result.message
        );
    }

    // ── Multiple callbacks at same point in Hooks ──────────────────

    #[test]
    fn hook_runner_multiple_callbacks_at_same_point() {
        use std::sync::{Arc, Mutex};

        let log = Arc::new(Mutex::new(Vec::<String>::new()));

        let mut runner = Hooks::new();
        for i in 0..5 {
            let l = Arc::clone(&log);
            runner.register(
                format!("post_turn_{i}"),
                HookCallback::PostTurn(Box::new(move |_ctx| {
                    l.lock().unwrap().push(format!("post_turn_{i}"));
                })),
            );
        }

        runner.run_post_turn(&PostTurnContext {
            response_text: "response".into(),
            turn_number: 1,
        });

        let entries = log.lock().unwrap().clone();
        assert_eq!(
            entries,
            vec![
                "post_turn_0",
                "post_turn_1",
                "post_turn_2",
                "post_turn_3",
                "post_turn_4"
            ],
            "all 5 callbacks should fire in registration order"
        );
    }

    // ── Duplicate hook replacement in Hooks ────────────────────────

    #[test]
    fn hook_runner_duplicate_replaces_previous() {
        use std::sync::{
            Arc,
            atomic::{AtomicU32, Ordering},
        };

        let counter = Arc::new(AtomicU32::new(0));

        let mut runner = Hooks::new();

        // Register a hook that adds 10.
        let c1 = Arc::clone(&counter);
        runner.register(
            "counter_hook",
            HookCallback::PreTurn(Box::new(move |_ctx| {
                c1.fetch_add(10, Ordering::SeqCst);
            })),
        );

        // Register a hook with the SAME name and SAME point — should replace.
        let c2 = Arc::clone(&counter);
        runner.register(
            "counter_hook",
            HookCallback::PreTurn(Box::new(move |_ctx| {
                c2.fetch_add(1, Ordering::SeqCst);
            })),
        );

        runner.run_pre_turn(&PreTurnContext {
            prompt: "test".into(),
            turn_number: 1,
        });

        // Only the replacement (adds 1) should have run, not the original (adds 10).
        let value = counter.load(Ordering::SeqCst);
        assert_eq!(
            value, 1,
            "duplicate hook should replace the previous; expected 1 but got {value}"
        );
    }

    // ── Convenience builder tests ───────────────────────────────────────

    #[test]
    fn convenience_on_pre_turn() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };

        let fired = Arc::new(AtomicBool::new(false));
        let f = Arc::clone(&fired);

        let mut runner = Hooks::new();
        runner.on_pre_turn("test", move |_ctx| {
            f.store(true, Ordering::SeqCst);
        });
        runner.run_pre_turn(&PreTurnContext {
            prompt: "hi".into(),
            turn_number: 1,
        });
        assert!(fired.load(Ordering::SeqCst));
    }

    #[test]
    fn convenience_on_post_turn() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };

        let fired = Arc::new(AtomicBool::new(false));
        let f = Arc::clone(&fired);

        let mut runner = Hooks::new();
        runner.on_post_turn("test", move |_ctx| {
            f.store(true, Ordering::SeqCst);
        });
        runner.run_post_turn(&PostTurnContext {
            response_text: "ok".into(),
            turn_number: 1,
        });
        assert!(fired.load(Ordering::SeqCst));
    }

    #[test]
    fn convenience_on_pre_tool_call_decide() {
        let mut runner = Hooks::new();
        runner.on_pre_tool_call_decide("gate", |ctx| {
            if ctx.tool_name == "blocked" {
                HookResult::deny("nope")
            } else {
                HookResult::allow()
            }
        });

        let allowed = runner.run_pre_tool_call_decide(&PreToolCallDecideContext {
            tool_name: "ok_tool".into(),
            tool_args: serde_json::Value::Null,
        });
        assert!(allowed.allow);

        let denied = runner.run_pre_tool_call_decide(&PreToolCallDecideContext {
            tool_name: "blocked".into(),
            tool_args: serde_json::Value::Null,
        });
        assert!(!denied.allow);
    }

    #[test]
    fn convenience_on_post_tool_call() {
        use std::sync::{Arc, Mutex};

        let captured = Arc::new(Mutex::new(String::new()));
        let c = Arc::clone(&captured);

        let mut runner = Hooks::new();
        runner.on_post_tool_call("logger", move |ctx| {
            *c.lock().unwrap() = ctx.result.clone();
        });
        runner.run_post_tool_call(&PostToolCallContext {
            tool_name: "t".into(),
            tool_args: serde_json::Value::Null,
            result: "done".into(),
        });
        assert_eq!(*captured.lock().unwrap(), "done");
    }

    #[test]
    fn convenience_on_tool_error() {
        use std::sync::{Arc, Mutex};

        let captured = Arc::new(Mutex::new(String::new()));
        let c = Arc::clone(&captured);

        let mut runner = Hooks::new();
        runner.on_tool_error("err_log", move |ctx| {
            *c.lock().unwrap() = ctx.error.clone();
        });
        runner.run_on_tool_error(&OnToolErrorContext {
            tool_name: "t".into(),
            tool_args: serde_json::Value::Null,
            error: "boom".into(),
        });
        assert_eq!(*captured.lock().unwrap(), "boom");
    }

    #[test]
    fn convenience_on_compaction() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };

        let fired = Arc::new(AtomicBool::new(false));
        let f = Arc::clone(&fired);

        let mut runner = Hooks::new();
        runner.on_compaction("compact_log", move |_ctx| {
            f.store(true, Ordering::SeqCst);
        });
        runner.run_on_compaction(&OnCompactionContext {});
        assert!(fired.load(Ordering::SeqCst));
    }

    #[test]
    fn convenience_on_interaction() {
        let mut runner = Hooks::new();
        runner.on_interaction("filter", |ctx| {
            if ctx.message.contains("spam") {
                HookResult::deny("spam detected")
            } else {
                HookResult::allow()
            }
        });

        let allowed = runner.run_on_interaction(&OnInteractionContext {
            message: "hello".into(),
        });
        assert!(allowed.allow);

        let denied = runner.run_on_interaction(&OnInteractionContext {
            message: "this is spam".into(),
        });
        assert!(!denied.allow);
    }

    #[test]
    fn convenience_on_session_start() {
        use std::sync::{Arc, Mutex};

        let captured_id = Arc::new(Mutex::new(String::new()));
        let c = Arc::clone(&captured_id);

        let mut runner = Hooks::new();
        runner.on_session_start("log_start", move |ctx| {
            *c.lock().unwrap() = ctx.session.session_id.clone();
        });
        runner.run_on_session_start(&OnSessionStartContext {
            session: SessionContext {
                session_id: "sess-42".into(),
                agent_id: 7,
                started_at: Instant::now(),
            },
        });
        assert_eq!(*captured_id.lock().unwrap(), "sess-42");
    }

    #[test]
    fn convenience_on_session_end() {
        use std::sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        };

        let captured_agent = Arc::new(AtomicU64::new(0));
        let c = Arc::clone(&captured_agent);

        let mut runner = Hooks::new();
        runner.on_session_end("log_end", move |ctx| {
            c.store(ctx.session.agent_id, Ordering::SeqCst);
        });
        runner.run_on_session_end(&OnSessionEndContext {
            session: SessionContext {
                session_id: "sess-99".into(),
                agent_id: 42,
                started_at: Instant::now(),
            },
        });
        assert_eq!(captured_agent.load(Ordering::SeqCst), 42);
    }

    // ── TransformToolInput tests ────────────────────────────────────────

    #[test]
    fn transform_tool_input_modifies_args() {
        let mut runner = Hooks::new();
        runner.on_transform_tool_input("inject_flag", |ctx| {
            let mut args = ctx.tool_args.clone();
            if let Some(obj) = args.as_object_mut() {
                obj.insert("injected".to_owned(), serde_json::Value::Bool(true));
            }
            Some(args)
        });

        let result = runner.run_transform_tool_input(&PreToolCallDecideContext {
            tool_name: "some_tool".into(),
            tool_args: serde_json::json!({"key": "value"}),
        });
        assert_eq!(result["key"], "value");
        assert_eq!(result["injected"], true);
    }

    #[test]
    fn transform_tool_input_none_keeps_original() {
        let mut runner = Hooks::new();
        runner.on_transform_tool_input("noop", |_ctx| None);

        let original = serde_json::json!({"key": "value"});
        let result = runner.run_transform_tool_input(&PreToolCallDecideContext {
            tool_name: "t".into(),
            tool_args: original.clone(),
        });
        assert_eq!(result, original);
    }

    #[test]
    fn transform_tool_input_chains_multiple() {
        let mut runner = Hooks::new();

        // First transform: add field_a
        runner.on_transform_tool_input("add_a", |ctx| {
            let mut args = ctx.tool_args.clone();
            if let Some(obj) = args.as_object_mut() {
                obj.insert("a".to_owned(), serde_json::json!(1));
            }
            Some(args)
        });

        // Second transform: add field_b
        runner.on_transform_tool_input("add_b", |ctx| {
            let mut args = ctx.tool_args.clone();
            if let Some(obj) = args.as_object_mut() {
                obj.insert("b".to_owned(), serde_json::json!(2));
            }
            Some(args)
        });

        let result = runner.run_transform_tool_input(&PreToolCallDecideContext {
            tool_name: "t".into(),
            tool_args: serde_json::json!({}),
        });
        assert_eq!(result["a"], 1);
        assert_eq!(result["b"], 2);
    }

    #[test]
    fn transform_tool_input_panic_recovery() {
        let mut runner = Hooks::new();
        runner.on_transform_tool_input("panicker", |_ctx| {
            panic!("intentional test panic in transform hook");
        });

        let original = serde_json::json!({"safe": true});
        let result = runner.run_transform_tool_input(&PreToolCallDecideContext {
            tool_name: "t".into(),
            tool_args: original.clone(),
        });
        // After panic, original args should be preserved.
        assert_eq!(result, original);
    }

    #[test]
    fn transform_callback_debug_format() {
        let cb = HookCallback::TransformToolInput(Box::new(|_| None));
        let dbg = format!("{cb:?}");
        assert_eq!(dbg, "HookCallback::transform_tool_input");
    }

    // ── Builder chaining test ───────────────────────────────────────────

    #[test]
    fn convenience_builders_chain() {
        use std::sync::{
            Arc,
            atomic::{AtomicU32, Ordering},
        };

        let counter = Arc::new(AtomicU32::new(0));
        let c1 = Arc::clone(&counter);
        let c2 = Arc::clone(&counter);

        let mut runner = Hooks::new();
        runner
            .on_pre_turn("a", move |_ctx| {
                c1.fetch_add(1, Ordering::SeqCst);
            })
            .on_pre_turn("b", move |_ctx| {
                c2.fetch_add(10, Ordering::SeqCst);
            });

        runner.run_pre_turn(&PreTurnContext {
            prompt: "test".into(),
            turn_number: 1,
        });
        assert_eq!(counter.load(Ordering::SeqCst), 11);
    }

    #[test]
    fn hooks_fluent_chaining() {
        use std::sync::{
            Arc,
            atomic::{AtomicU32, Ordering},
        };

        let counter = Arc::new(AtomicU32::new(0));
        let c1 = Arc::clone(&counter);
        let c2 = Arc::clone(&counter);

        let hooks = Hooks::new()
            .with_pre_turn("a", move |_ctx| {
                c1.fetch_add(1, Ordering::SeqCst);
            })
            .with_post_turn("b", move |_ctx| {
                c2.fetch_add(10, Ordering::SeqCst);
            });

        hooks.run_pre_turn(&PreTurnContext {
            prompt: "hi".into(),
            turn_number: 1,
        });
        hooks.run_post_turn(&PostTurnContext {
            response_text: "ok".into(),
            turn_number: 1,
        });
        assert_eq!(counter.load(Ordering::SeqCst), 11);
    }
}

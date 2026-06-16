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
#[path = "runner_tests.rs"]
mod tests;

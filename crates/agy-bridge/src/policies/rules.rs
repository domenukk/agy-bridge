//! Policy rules, decisions, and policy sets.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::Error;

/// A single policy rule controlling tool access.
///
/// These map 1:1 to the Antigravity SDK's policy helpers:
///
/// | Rust variant            | Python SDK call                        |
/// |-------------------------|----------------------------------------|
/// | `Allow(tool)`           | `policy.allow("tool")`                 |
/// | `Deny(tool)`            | `policy.deny("tool")`                  |
/// | `AllowAll`              | `policy.allow("*")`                    |
/// | `DenyAll`               | `policy.deny("*")`                     |
/// | `WorkspaceOnly(paths)`  | ``policy.workspace_only(["/path/..."])`` |
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PolicyRule {
    /// Allow a specific tool by name (e.g. `"view_file"`).
    Allow(String),
    /// Deny a specific tool by name (e.g. `"run_command"`).
    Deny(String),
    /// Allow all tools.
    AllowAll,
    /// Deny all tools.
    DenyAll,
    /// Ask the user before running a specific tool by name (e.g. `"run_command"`).
    AskUser { tool: String, handler_id: String },
    /// Restrict file-access tools to the given workspace directories.
    WorkspaceOnly(Vec<PathBuf>),
}

impl PolicyRule {
    /// Create an [`Allow`](Self::Allow) rule for the given tool name.
    ///
    /// # Examples
    ///
    /// ```
    /// # use agy_bridge::policies::PolicyRule;
    /// let rule = PolicyRule::allow("view_file");
    /// ```
    #[must_use]
    pub fn allow(tool: impl Into<String>) -> Self {
        Self::Allow(tool.into())
    }

    /// Create a [`Deny`](Self::Deny) rule for the given tool name.
    ///
    /// # Examples
    ///
    /// ```
    /// # use agy_bridge::policies::PolicyRule;
    /// let rule = PolicyRule::deny("run_command");
    /// ```
    #[must_use]
    pub fn deny(tool: impl Into<String>) -> Self {
        Self::Deny(tool.into())
    }

    /// Create an [`AllowAll`](Self::AllowAll) rule.
    ///
    /// # Examples
    ///
    /// ```
    /// # use agy_bridge::policies::PolicyRule;
    /// let rule = PolicyRule::allow_all();
    /// ```
    #[must_use]
    pub const fn allow_all() -> Self {
        Self::AllowAll
    }

    /// Create a [`DenyAll`](Self::DenyAll) rule.
    ///
    /// # Examples
    ///
    /// ```
    /// # use agy_bridge::policies::PolicyRule;
    /// let rule = PolicyRule::deny_all();
    /// ```
    #[must_use]
    pub const fn deny_all() -> Self {
        Self::DenyAll
    }

    /// Create a [`WorkspaceOnly`](Self::WorkspaceOnly) rule.
    ///
    /// # Examples
    ///
    /// ```
    /// # use agy_bridge::policies::PolicyRule;
    /// let rule = PolicyRule::workspace_only(["/my/project"]);
    /// ```
    #[must_use]
    pub fn workspace_only(paths: impl IntoIterator<Item = impl Into<PathBuf>>) -> Self {
        Self::WorkspaceOnly(paths.into_iter().map(Into::into).collect())
    }

    /// Human-readable description used for logging and diagnostics.
    #[must_use]
    pub fn description(&self) -> String {
        match self {
            Self::Allow(tool) => format!("allow({tool})"),
            Self::Deny(tool) => format!("deny({tool})"),
            Self::AllowAll => "allow(*)".to_owned(),
            Self::DenyAll => "deny(*)".to_owned(),
            Self::AskUser { tool, handler_id } => format!("ask_user({tool}, handler={handler_id})"),
            Self::WorkspaceOnly(paths) => {
                let joined: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
                format!("workspace_only([{}])", joined.join(", "))
            }
        }
    }

    /// Validate that the policy rule is well-formed.
    ///
    /// # Rules
    ///
    /// - [`Allow`](Self::Allow) / [`Deny`](Self::Deny): tool name must not be
    ///   empty or whitespace-only.
    /// - [`AskUser`](Self::AskUser): both `tool` and `handler_id` must be
    ///   non-empty.
    /// - [`WorkspaceOnly`](Self::WorkspaceOnly): must contain at least one path.
    /// - [`AllowAll`](Self::AllowAll) / [`DenyAll`](Self::DenyAll): always valid.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidConfig`] if validation fails.
    pub fn validate(&self) -> Result<(), Error> {
        match self {
            Self::Allow(tool) | Self::Deny(tool) => {
                if tool.trim().is_empty() {
                    return Err(Error::InvalidConfig {
                        message: "PolicyRule tool name must not be empty".to_owned(),
                    });
                }
            }
            Self::AskUser { tool, handler_id } => {
                if tool.trim().is_empty() {
                    return Err(Error::InvalidConfig {
                        message: "PolicyRule::AskUser tool name must not be empty".to_owned(),
                    });
                }
                if handler_id.trim().is_empty() {
                    return Err(Error::InvalidConfig {
                        message: format!("PolicyRule::AskUser '{tool}' has an empty handler_id"),
                    });
                }
            }
            Self::WorkspaceOnly(paths) => {
                if paths.is_empty() {
                    return Err(Error::InvalidConfig {
                        message: "PolicyRule::WorkspaceOnly must contain at least one path"
                            .to_owned(),
                    });
                }
            }
            Self::AllowAll | Self::DenyAll => {}
        }
        Ok(())
    }
}

/// Create an [`AskUser`](PolicyRule::AskUser) policy rule for a tool,
/// which prompts the user for confirmation before execution.
///
/// # Examples
///
/// ```
/// # use agy_bridge::policies::ask_user;
/// let rule = ask_user("run_command", "confirm_run_command");
/// ```
#[must_use]
pub fn ask_user(tool: impl Into<String>, handler_id: impl Into<String>) -> PolicyRule {
    PolicyRule::AskUser {
        tool: tool.into(),
        handler_id: handler_id.into(),
    }
}

/// Creates a policy that confirms before running shell commands.
///
/// Equivalent to `PolicyRule::AskUser { tool: "run_command", handler_id: "confirm_run_command" }`.
///
/// # Examples
///
/// ```
/// # use agy_bridge::policies::confirm_run_command;
/// let rule = confirm_run_command();
/// ```
#[must_use]
pub fn confirm_run_command() -> PolicyRule {
    ask_user("run_command", "confirm_run_command")
}

/// Returns a safe default policy set: `AllowAll` for read operations,
/// `AskUser` for write/execute operations, and `DenyAll` as a fallback.
///
/// Specifically:
/// - **Allow**: `view_file`, `read_file`, `list_dir`, `search`
/// - **`AskUser`**: `run_command`, `write_file`, `edit_file`
/// - **`DenyAll`**: everything else
///
/// # Panics
///
/// Panics if the statically defined default rules fail validation.
///
/// # Examples
///
/// ```
/// # use agy_bridge::policies::safe_defaults;
/// let policies = safe_defaults();
/// assert!(policies.evaluate("view_file").is_allowed());
/// assert!(policies.evaluate("run_command").needs_confirmation());
/// assert!(policies.evaluate("unknown_tool").is_denied());
/// ```
#[must_use]
pub fn safe_defaults() -> PolicySet {
    const READ_TOOLS: &[&str] = &["view_file", "read_file", "list_dir", "search"];
    const WRITE_TOOLS: &[(&str, &str)] = &[
        ("run_command", "confirm_run_command"),
        ("write_file", "confirm_write_file"),
        ("edit_file", "confirm_edit_file"),
    ];

    let mut set = PolicySet::new();
    for tool in READ_TOOLS {
        // Safety: these are known-good tool names, unwrap is fine.
        set.push(PolicyRule::Allow((*tool).to_owned()))
            .expect("safe_defaults: valid Allow rule");
    }
    for (tool, handler_id) in WRITE_TOOLS {
        set.push(PolicyRule::AskUser {
            tool: (*tool).to_owned(),
            handler_id: (*handler_id).to_owned(),
        })
        .expect("safe_defaults: valid AskUser rule");
    }
    set.push(PolicyRule::DenyAll)
        .expect("safe_defaults: valid DenyAll rule");
    set
}

/// Trait for handlers that confirm interactive tool-execution prompts.
///
/// Implementors decide whether a tool call should proceed based on its
/// name and arguments.  This is used when a [`PolicyRule::AskUser`]
/// rule matches a tool call.
///
/// # Examples
///
/// ```
/// use agy_bridge::policies::AskUserHandler;
///
/// struct AlwaysConfirm;
/// impl AskUserHandler for AlwaysConfirm {
///     fn confirm(&self, _tool_name: &str, _tool_args: &serde_json::Value) -> bool {
///         true
///     }
/// }
/// ```
pub trait AskUserHandler: Send + Sync {
    /// Decide whether the tool call should proceed.
    ///
    /// Return `true` to allow execution, `false` to deny.
    fn confirm(&self, tool_name: &str, tool_args: &serde_json::Value) -> bool;
}

/// The outcome of evaluating a [`PolicySet`] for a given tool.
///
/// Unlike a plain `bool`, this enum distinguishes between unconditional
/// allow/deny and the case where a tool requires interactive user
/// confirmation before it can proceed.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyDecision<'a> {
    /// The tool is unconditionally allowed.
    Allow,
    /// The tool is unconditionally denied.
    Deny,
    /// The tool requires user confirmation before execution.
    ///
    /// `handler_id` identifies the confirmation handler that should be
    /// consulted (e.g. a UI dialog or an IPC callback).
    NeedsConfirmation {
        /// Opaque identifier for the confirmation handler.
        handler_id: &'a str,
    },
}

impl PolicyDecision<'_> {
    /// Returns `true` only if the decision is [`Allow`](Self::Allow).
    ///
    /// [`NeedsConfirmation`](Self::NeedsConfirmation) is **not** considered
    /// allowed — callers must explicitly handle it.
    #[must_use]
    pub const fn is_allowed(&self) -> bool {
        matches!(self, Self::Allow)
    }

    /// Returns `true` if the decision is [`Deny`](Self::Deny).
    #[must_use]
    pub const fn is_denied(&self) -> bool {
        matches!(self, Self::Deny)
    }

    /// Returns `true` if the decision is
    /// [`NeedsConfirmation`](Self::NeedsConfirmation).
    #[must_use]
    pub const fn needs_confirmation(&self) -> bool {
        matches!(self, Self::NeedsConfirmation { .. })
    }
}

/// An ordered list of policy rules applied to an agent.
///
/// Policies are evaluated in order — the first matching rule wins.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PolicySet {
    rules: Vec<PolicyRule>,
}

impl PolicySet {
    /// Create an empty policy set.
    #[must_use]
    pub const fn new() -> Self {
        Self { rules: Vec::new() }
    }

    /// Add a policy rule to the end of the list.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidConfig`] if the rule fails validation
    /// (e.g. empty tool name or empty handler ID).
    pub fn push(&mut self, rule: PolicyRule) -> Result<(), Error> {
        rule.validate()?;
        self.rules.push(rule);
        Ok(())
    }

    /// Builder-style method to add a rule, returning `Self` for chaining.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidConfig`] if the rule fails validation.
    ///
    /// # Examples
    ///
    /// ```
    /// # use agy_bridge::policies::{PolicyRule, PolicySet};
    /// let set = PolicySet::new()
    ///     .with_rule(PolicyRule::allow("view_file"))?
    ///     .with_rule(PolicyRule::DenyAll)?;
    /// assert_eq!(set.len(), 2);
    /// # Ok::<(), agy_bridge::Error>(())
    /// ```
    pub fn with_rule(mut self, rule: PolicyRule) -> Result<Self, Error> {
        self.push(rule)?;
        Ok(self)
    }

    /// Iterate over the policy rules in evaluation order.
    pub fn iter(&self) -> impl Iterator<Item = &PolicyRule> {
        self.rules.iter()
    }

    /// Number of rules.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.rules.len()
    }

    /// Whether the set is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Evaluate whether a tool call is allowed by this policy set.
    ///
    /// Uses first-match semantics: the first rule that matches the tool name
    /// determines the result.  Returns [`PolicyDecision::Deny`] if no rule
    /// matches (default-deny).
    ///
    /// [`WorkspaceOnly`](PolicyRule::WorkspaceOnly) rules are skipped during
    /// tool-name evaluation — they apply to path checks, not tool dispatch.
    #[must_use]
    pub fn evaluate(&self, tool_name: &str) -> PolicyDecision<'_> {
        for rule in &self.rules {
            match rule {
                PolicyRule::Allow(name) if name == tool_name => return PolicyDecision::Allow,
                PolicyRule::Deny(name) if name == tool_name => return PolicyDecision::Deny,
                PolicyRule::AllowAll => return PolicyDecision::Allow,
                PolicyRule::DenyAll => return PolicyDecision::Deny,
                PolicyRule::AskUser { tool, handler_id } if tool == tool_name => {
                    return PolicyDecision::NeedsConfirmation { handler_id };
                }
                PolicyRule::AskUser { .. }
                | PolicyRule::Allow(_)
                | PolicyRule::Deny(_)
                | PolicyRule::WorkspaceOnly(_) => {}
            }
        }
        PolicyDecision::Deny
    }
}

impl From<Vec<PolicyRule>> for PolicySet {
    /// Convert a `Vec<PolicyRule>` into a `PolicySet`.
    ///
    /// # Panics
    ///
    /// Panics if any rule fails validation (e.g. empty tool name,
    /// empty handler ID, or empty workspace list).
    /// Prefer [`PolicySet::validated_from`] for fallible
    /// conversion.
    fn from(rules: Vec<PolicyRule>) -> Self {
        Self::validated_from(rules).expect("PolicySet::from(Vec<PolicyRule>): invalid rules")
    }
}

impl FromIterator<PolicyRule> for PolicySet {
    /// Create a `PolicySet` from an iterator of policy rules,
    /// panic-validating them.
    ///
    /// # Panics
    ///
    /// Panics if any rule fails validation (e.g. empty tool name,
    /// empty handler ID, or empty workspace list).
    /// Use [`PolicySet::validated_from`] for fallible
    /// alternatives.
    fn from_iter<T: IntoIterator<Item = PolicyRule>>(iter: T) -> Self {
        let rules = iter.into_iter().collect::<Vec<_>>();
        Self::from(rules)
    }
}

impl<const N: usize> From<[PolicyRule; N]> for PolicySet {
    /// Create a `PolicySet` from a fixed-size array, panic-validating
    /// each rule.
    ///
    /// # Panics
    ///
    /// Panics if any rule fails validation (e.g. empty tool name,
    /// empty handler ID, or empty workspace list).
    /// Prefer [`PolicySet::validated_from`] for fallible conversion.
    fn from(rules: [PolicyRule; N]) -> Self {
        Self::from(Vec::from(rules))
    }
}

impl PolicySet {
    /// Create a [`PolicySet`] from a `Vec` of rules, **validating each one**.
    ///
    /// Returns the first validation error encountered, if any.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidConfig`] if any rule fails validation
    /// (e.g. empty tool name, empty handler ID, or empty workspace list).
    ///
    /// # Examples
    ///
    /// ```
    /// # use agy_bridge::policies::{PolicyRule, PolicySet};
    /// let rules = vec![
    ///     PolicyRule::allow("view_file"),
    ///     PolicyRule::deny("run_command"),
    ///     PolicyRule::DenyAll,
    /// ];
    /// let set = PolicySet::validated_from(rules).expect("valid rules");
    /// assert_eq!(set.len(), 3);
    /// ```
    pub fn validated_from(rules: Vec<PolicyRule>) -> Result<Self, Error> {
        for rule in &rules {
            rule.validate()?;
        }
        Ok(Self { rules })
    }
}

/// Iterate over the policy rules in evaluation order.
///
/// Enables `for rule in &policy_set { ... }` syntax.
impl<'a> IntoIterator for &'a PolicySet {
    type Item = &'a PolicyRule;
    type IntoIter = std::slice::Iter<'a, PolicyRule>;

    fn into_iter(self) -> Self::IntoIter {
        self.rules.iter()
    }
}

impl IntoIterator for PolicySet {
    type Item = PolicyRule;
    type IntoIter = std::vec::IntoIter<PolicyRule>;

    fn into_iter(self) -> Self::IntoIter {
        self.rules.into_iter()
    }
}

impl From<PolicySet> for Vec<PolicyRule> {
    fn from(set: PolicySet) -> Self {
        set.rules
    }
}

impl From<&PolicySet> for Vec<PolicyRule> {
    fn from(set: &PolicySet) -> Self {
        set.rules.clone()
    }
}

#[cfg(test)]
#[path = "rules_tests.rs"]
mod tests;

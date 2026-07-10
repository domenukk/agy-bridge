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
mod tests {
    use std::path::PathBuf;

    use super::{
        super::path::{is_path_in_workspace, normalize_path},
        *,
    };

    #[test]
    fn deny_policy_description() {
        let rule = PolicyRule::Deny("run_command".to_owned());
        assert_eq!(rule.description(), "deny(run_command)");
    }

    #[test]
    fn allow_policy_description() {
        let rule = PolicyRule::Allow("view_file".to_owned());
        assert_eq!(rule.description(), "allow(view_file)");
    }

    #[test]
    fn allow_all_description() {
        assert_eq!(PolicyRule::AllowAll.description(), "allow(*)");
    }

    #[test]
    fn deny_all_description() {
        assert_eq!(PolicyRule::DenyAll.description(), "deny(*)");
    }

    #[test]
    fn workspace_only_description() {
        let rule = PolicyRule::WorkspaceOnly(vec![
            PathBuf::from("/workspace/a"),
            PathBuf::from("/workspace/b"),
        ]);
        assert_eq!(
            rule.description(),
            "workspace_only([/workspace/a, /workspace/b])"
        );
    }

    #[test]
    fn policy_set_operations() {
        let mut set = PolicySet::new();
        assert!(set.is_empty());

        set.push(PolicyRule::DenyAll).unwrap();
        set.push(PolicyRule::Allow("view_file".to_owned())).unwrap();

        assert_eq!(set.len(), 2);
        let descriptions: Vec<String> = set.iter().map(PolicyRule::description).collect();
        assert_eq!(descriptions, vec!["deny(*)", "allow(view_file)"]);
    }

    #[test]
    fn policy_set_from_vec() {
        let set = PolicySet::from(vec![
            PolicyRule::AllowAll,
            PolicyRule::Deny("run_command".to_owned()),
        ]);
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn policy_rule_serde_roundtrip() {
        let rules = vec![
            PolicyRule::Allow("view_file".to_owned()),
            PolicyRule::Deny("run_command".to_owned()),
            PolicyRule::AllowAll,
            PolicyRule::DenyAll,
            PolicyRule::WorkspaceOnly(vec![PathBuf::from("/tmp")]),
            PolicyRule::AskUser {
                tool: "run_command".to_owned(),
                handler_id: "handler-1".to_owned(),
            },
        ];
        for rule in &rules {
            let json = serde_json::to_string(rule).expect("serialize");
            let parsed: PolicyRule = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(&parsed, rule);
        }
    }

    #[test]
    fn verify_deny_blocks_tool_calls() {
        // Simulated policy evaluation: first matching rule wins.
        let set = PolicySet::from(vec![
            PolicyRule::Deny("run_command".to_owned()),
            PolicyRule::AllowAll,
        ]);

        let tool_name = "run_command";
        let decision = set.evaluate(tool_name);
        assert!(decision.is_denied(), "run_command should be denied");

        let decision = set.evaluate("view_file");
        assert!(decision.is_allowed(), "view_file should be allowed");
    }

    #[test]
    fn verify_workspace_only_restricts_access() {
        let _set = PolicySet::from(vec![PolicyRule::WorkspaceOnly(vec![PathBuf::from(
            "/workspace",
        )])]);

        // A path inside the workspace should be allowed.
        assert!(is_path_in_workspace(
            "/workspace/src/main.rs",
            &[PathBuf::from("/workspace")]
        ));

        // A path outside the workspace should be denied.
        assert!(!is_path_in_workspace(
            "/etc/passwd",
            &[PathBuf::from("/workspace")]
        ));
    }

    #[test]
    fn policy_set_serde_roundtrip() {
        let set = PolicySet::from(vec![
            PolicyRule::DenyAll,
            PolicyRule::Allow("view_file".to_owned()),
            PolicyRule::WorkspaceOnly(vec![PathBuf::from("/ws")]),
        ]);
        let json = serde_json::to_string(&set).expect("serialize");
        let parsed: PolicySet = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.len(), 3);
        let descriptions: Vec<String> = parsed.iter().map(PolicyRule::description).collect();
        assert_eq!(
            descriptions,
            vec!["deny(*)", "allow(view_file)", "workspace_only([/ws])"]
        );
    }

    #[test]
    fn policy_set_from_conversions() {
        let set = PolicySet::from(vec![PolicyRule::Allow("view_file".to_owned())]);

        let vec_from_owned: Vec<PolicyRule> = Vec::from(set.clone());
        assert_eq!(vec_from_owned.len(), 1);
        assert_eq!(vec_from_owned[0], PolicyRule::Allow("view_file".to_owned()));

        let vec_from_ref: Vec<PolicyRule> = Vec::from(&set);
        assert_eq!(vec_from_ref.len(), 1);
        assert_eq!(vec_from_ref[0], PolicyRule::Allow("view_file".to_owned()));

        let set_from_arr = PolicySet::from([PolicyRule::Allow("view_file".to_owned())]);
        assert_eq!(set_from_arr.len(), 1);

        let set_from_vec = PolicySet::from(vec![PolicyRule::Allow("view_file".to_owned())]);
        assert_eq!(set_from_vec.len(), 1);
    }

    #[test]
    fn policy_set_default_is_empty() {
        let set = PolicySet::default();
        assert!(set.is_empty());
        assert_eq!(set.len(), 0);
    }

    #[test]
    fn evaluate_empty_policy_set_denies() {
        let set = PolicySet::new();
        assert!(set.evaluate("view_file").is_denied());
        assert!(set.evaluate("run_command").is_denied());
    }

    #[test]
    fn evaluate_deny_all_first_blocks_everything() {
        let set = PolicySet::from(vec![
            PolicyRule::DenyAll,
            PolicyRule::Allow("view_file".to_owned()),
        ]);
        // DenyAll is first, so even explicitly allowed tools are denied.
        assert!(set.evaluate("view_file").is_denied());
        assert!(set.evaluate("run_command").is_denied());
    }

    #[test]
    fn evaluate_allow_all_only() {
        let set = PolicySet::from(vec![PolicyRule::AllowAll]);
        assert!(set.evaluate("view_file").is_allowed());
        assert!(set.evaluate("run_command").is_allowed());
        assert!(set.evaluate("anything").is_allowed());
    }

    #[test]
    fn evaluate_specific_allow_before_deny_all() {
        let set = PolicySet::from(vec![
            PolicyRule::Allow("view_file".to_owned()),
            PolicyRule::DenyAll,
        ]);
        assert!(set.evaluate("view_file").is_allowed());
        assert!(set.evaluate("run_command").is_denied());
    }

    #[test]
    fn evaluate_ask_user_returns_needs_confirmation() {
        let set = PolicySet::from(vec![
            super::ask_user("run_command", "confirm_run_command"),
            PolicyRule::allow_all(),
        ]);
        let decision = set.evaluate("run_command");
        assert!(decision.needs_confirmation());
        assert!(!decision.is_allowed());
        assert!(!decision.is_denied());
        // Other tools fall through to AllowAll.
        assert!(set.evaluate("view_file").is_allowed());
    }

    #[test]
    fn evaluate_ask_user_handler_id_preserved() {
        let set = PolicySet::from(vec![super::ask_user("rm", "my_handler")]);
        match set.evaluate("rm") {
            PolicyDecision::NeedsConfirmation { handler_id } => {
                assert_eq!(handler_id, "my_handler");
            }
            other => panic!("expected NeedsConfirmation, got {other:?}"),
        }
    }

    #[test]
    fn confirm_run_command_helper() {
        let rule = super::confirm_run_command();
        assert_eq!(
            rule,
            PolicyRule::AskUser {
                tool: "run_command".to_owned(),
                handler_id: "confirm_run_command".to_owned(),
            }
        );
    }

    // ── Builder method tests ──────────────────────────────────────

    #[test]
    fn builder_allow_all() {
        assert_eq!(PolicyRule::allow_all(), PolicyRule::AllowAll);
    }

    #[test]
    fn builder_deny_all() {
        assert_eq!(PolicyRule::deny_all(), PolicyRule::DenyAll);
    }

    #[test]
    fn builder_workspace_only() {
        let rule = PolicyRule::workspace_only(["/ws/a", "/ws/b"]);
        assert_eq!(
            rule,
            PolicyRule::WorkspaceOnly(vec![PathBuf::from("/ws/a"), PathBuf::from("/ws/b"),])
        );
    }

    #[test]
    fn builder_allow_deny_accept_str() {
        // Verify impl Into<String> works with &str.
        let _ = PolicyRule::allow("view_file");
        let _ = PolicyRule::deny("run_command");
    }

    #[test]
    fn workspace_only_empty_paths_denies_all() {
        let rule = PolicyRule::WorkspaceOnly(vec![]);
        assert_eq!(rule.description(), "workspace_only([])");
        assert!(!is_path_in_workspace("/anything", &[]));
    }

    #[test]
    fn workspace_path_exact_root_match() {
        let workspaces = [PathBuf::from("/workspace")];
        assert!(is_path_in_workspace("/workspace", &workspaces));
        assert!(is_path_in_workspace(
            "/workspace/deep/nested/file.rs",
            &workspaces
        ));
        assert!(!is_path_in_workspace("/workspace2/file.rs", &workspaces));
    }

    #[test]
    fn workspace_multiple_roots() {
        let workspaces = [PathBuf::from("/ws1"), PathBuf::from("/ws2")];
        assert!(is_path_in_workspace("/ws1/a.rs", &workspaces));
        assert!(is_path_in_workspace("/ws2/b.rs", &workspaces));
        assert!(!is_path_in_workspace("/ws3/c.rs", &workspaces));
    }

    #[test]
    fn policy_rule_equality() {
        assert_eq!(PolicyRule::AllowAll, PolicyRule::AllowAll);
        assert_eq!(PolicyRule::DenyAll, PolicyRule::DenyAll);
        assert_ne!(PolicyRule::AllowAll, PolicyRule::DenyAll);
        assert_eq!(
            PolicyRule::Allow("x".to_owned()),
            PolicyRule::Allow("x".to_owned())
        );
        assert_ne!(
            PolicyRule::Allow("x".to_owned()),
            PolicyRule::Allow("y".to_owned())
        );
    }

    /// Security test §7.4.7: Systematically attempt to bypass policy rules.
    /// Verify that denied tools are always rejected regardless of strategy:
    /// - Explicitly denied tool in a mixed allow/deny set
    /// - `DenyAll` shadowing a later Allow
    /// - Tools not mentioned in an allow-list are denied
    /// - `WorkspaceOnly` does not implicitly allow tool execution
    /// - Path traversal attempts outside workspace
    #[test]
    fn security_policy_bypass_attempts() {
        // Scenario 1: Explicitly denied tool followed by AllowAll.
        // The deny should win because first-match takes priority.
        let set = PolicySet::from(vec![
            PolicyRule::Deny("run_command".to_owned()),
            PolicyRule::AllowAll,
        ]);
        assert!(
            set.evaluate("run_command").is_denied(),
            "explicitly denied tool must be rejected even with AllowAll after"
        );
        assert!(
            set.evaluate("view_file").is_allowed(),
            "non-denied tool should be allowed"
        );

        // Scenario 2: DenyAll overrides later specific Allow.
        let set = PolicySet::from(vec![
            PolicyRule::DenyAll,
            PolicyRule::Allow("run_command".to_owned()),
        ]);
        assert!(
            set.evaluate("run_command").is_denied(),
            "DenyAll must shadow later Allow"
        );
        assert!(
            set.evaluate("view_file").is_denied(),
            "DenyAll must block everything"
        );

        // Scenario 3: Only specific tools allowed, unlisted tools denied.
        let set = PolicySet::from(vec![
            PolicyRule::Allow("view_file".to_owned()),
            PolicyRule::DenyAll,
        ]);
        assert!(set.evaluate("view_file").is_allowed());
        assert!(
            set.evaluate("run_command").is_denied(),
            "unlisted tool must be denied"
        );
        assert!(
            set.evaluate("create_file").is_denied(),
            "unlisted tool must be denied"
        );
        assert!(
            set.evaluate("edit_file").is_denied(),
            "unlisted tool must be denied"
        );

        // Scenario 4: Empty policy set denies everything (default deny).
        let empty = PolicySet::new();
        assert!(
            empty.evaluate("view_file").is_denied(),
            "empty policy must deny all tools"
        );
        assert!(
            empty.evaluate("run_command").is_denied(),
            "empty policy must deny all tools"
        );

        // Scenario 5: Paths that are genuinely outside the workspace root.
        let workspaces = [PathBuf::from("/workspace")];
        let outside_paths = [
            "/etc/passwd",
            "/tmp/scratch",
            "/home/user/documents/notes.sh",
            "/workspace2/other_project/file.rs",
        ];
        for path in &outside_paths {
            assert!(
                !is_path_in_workspace(path, &workspaces),
                "'{path}' must NOT match workspace root"
            );
        }

        // Paths legitimately inside the workspace.
        let inside_paths = [
            "/workspace/src/main.rs",
            "/workspace/deep/nested/dir/file.txt",
        ];
        for path in &inside_paths {
            assert!(
                is_path_in_workspace(path, &workspaces),
                "'{path}' should match workspace root"
            );
        }
    }

    // =========================================================================
    // Path canonicalization / traversal tests (R3)
    // =========================================================================

    #[test]
    fn traversal_dotdot_escapes_workspace() {
        let workspaces = [PathBuf::from("/workspace")];
        assert!(
            !is_path_in_workspace("/workspace/../etc/passwd", &workspaces),
            "path traversal via .. must be blocked"
        );
    }

    #[test]
    fn dot_segment_inside_workspace_is_accepted() {
        let workspaces = [PathBuf::from("/workspace")];
        assert!(
            is_path_in_workspace("/workspace/./subdir/file.rs", &workspaces),
            "path with . segment should resolve inside workspace"
        );
    }

    #[test]
    fn normalize_path_removes_dot() {
        let p = normalize_path(std::path::Path::new("/a/./b/c"));
        assert_eq!(p, PathBuf::from("/a/b/c"));
    }

    #[test]
    fn normalize_path_resolves_dotdot() {
        let p = normalize_path(std::path::Path::new("/a/b/../c"));
        assert_eq!(p, PathBuf::from("/a/c"));
    }

    #[test]
    fn normalize_path_dotdot_at_root_stays_at_root() {
        let p = normalize_path(std::path::Path::new("/../etc"));
        assert_eq!(p, PathBuf::from("/etc"));
    }

    #[test]
    fn normalize_path_multiple_dotdots() {
        let p = normalize_path(std::path::Path::new("/a/b/c/../../d"));
        assert_eq!(p, PathBuf::from("/a/d"));
    }

    #[test]
    fn normalize_path_already_clean() {
        let p = normalize_path(std::path::Path::new("/workspace/src/main.rs"));
        assert_eq!(p, PathBuf::from("/workspace/src/main.rs"));
    }

    #[test]
    fn normalize_path_only_root() {
        let p = normalize_path(std::path::Path::new("/"));
        assert_eq!(p, PathBuf::from("/"));
    }

    #[test]
    fn traversal_multiple_dotdots_escape() {
        let workspaces = [PathBuf::from("/workspace/sub")];
        // /workspace/sub/../../etc/shadow → /etc/shadow
        assert!(
            !is_path_in_workspace("/workspace/sub/../../etc/shadow", &workspaces),
            "double .. traversal must be blocked"
        );
    }

    #[test]
    fn traversal_dotdot_then_back_in() {
        let workspaces = [PathBuf::from("/workspace")];
        // /workspace/../workspace/file.rs → /workspace/file.rs — still inside!
        assert!(
            is_path_in_workspace("/workspace/../workspace/file.rs", &workspaces),
            "path that goes up then back into workspace should be allowed"
        );
    }

    #[test]
    fn workspace_root_with_trailing_dot() {
        let workspaces = [PathBuf::from("/workspace/.")];
        assert!(
            is_path_in_workspace("/workspace/src/lib.rs", &workspaces),
            "workspace root with trailing dot normalizes to /workspace"
        );
    }

    #[test]
    fn normalize_empty_components() {
        let p = normalize_path(std::path::Path::new("/a/./././b"));
        assert_eq!(p, PathBuf::from("/a/b"));
    }

    #[test]
    fn normalize_relative_path() {
        let p = normalize_path(std::path::Path::new("a/b/../c"));
        assert_eq!(p, PathBuf::from("a/c"));
    }

    #[test]
    fn traversal_relative_dotdot_above_start() {
        // Relative path: trying to go above start should stop.
        let p = normalize_path(std::path::Path::new("a/../../b"));
        assert_eq!(p, PathBuf::from("b"));
    }

    #[test]
    fn workspace_path_prefix_similarity_rejected() {
        // /workspace2 is NOT inside /workspace.
        let workspaces = [PathBuf::from("/workspace")];
        assert!(!is_path_in_workspace("/workspace2/file.rs", &workspaces));
    }

    #[test]
    fn workspace_with_dotdot_in_root_is_normalized() {
        // Workspace root itself has ..: /a/b/../c → /a/c
        let workspaces = [PathBuf::from("/a/b/../c")];
        assert!(is_path_in_workspace("/a/c/file.rs", &workspaces));
        assert!(!is_path_in_workspace("/a/b/file.rs", &workspaces));
    }

    // ── IntoIterator tests ──────────────────────────────────────────

    #[test]
    fn into_iter_policy_set_for_loop() {
        let set = PolicySet::from(vec![
            PolicyRule::Allow("view_file".to_owned()),
            PolicyRule::DenyAll,
        ]);

        let mut descriptions = Vec::new();
        for rule in &set {
            descriptions.push(rule.description());
        }
        assert_eq!(descriptions, vec!["allow(view_file)", "deny(*)"]);
    }

    #[test]
    fn into_iter_empty_policy_set() {
        let set = PolicySet::new();
        let count = (&set).into_iter().count();
        assert_eq!(count, 0);
    }

    #[test]
    fn into_iter_preserves_evaluation_order() {
        let set = PolicySet::from(vec![
            PolicyRule::Deny("run_command".to_owned()),
            PolicyRule::AllowAll,
            PolicyRule::DenyAll,
        ]);

        let rules: Vec<&PolicyRule> = (&set).into_iter().collect();
        assert_eq!(rules.len(), 3);
        assert_eq!(rules[0], &PolicyRule::Deny("run_command".to_owned()));
        assert_eq!(rules[1], &PolicyRule::AllowAll);
        assert_eq!(rules[2], &PolicyRule::DenyAll);
    }

    // ── PolicyRule validation tests ──────────────────────────────────

    #[test]
    fn validate_allow_empty_tool_name_is_err() {
        let rule = PolicyRule::Allow("  ".to_owned());
        assert!(rule.validate().is_err());
    }

    #[test]
    fn validate_deny_empty_tool_name_is_err() {
        let rule = PolicyRule::Deny(String::new());
        assert!(rule.validate().is_err());
    }

    #[test]
    fn validate_ask_user_empty_tool_is_err() {
        let rule = PolicyRule::AskUser {
            tool: "  ".to_owned(),
            handler_id: "handler".to_owned(),
        };
        assert!(rule.validate().is_err());
    }

    #[test]
    fn validate_ask_user_empty_handler_is_err() {
        let rule = PolicyRule::AskUser {
            tool: "run_command".to_owned(),
            handler_id: "  ".to_owned(),
        };
        assert!(rule.validate().is_err());
    }

    #[test]
    fn validate_workspace_only_empty_paths_is_err() {
        let rule = PolicyRule::WorkspaceOnly(vec![]);
        assert!(rule.validate().is_err());
    }

    #[test]
    fn validate_allow_all_is_ok() {
        assert!(PolicyRule::AllowAll.validate().is_ok());
    }

    #[test]
    fn validate_deny_all_is_ok() {
        assert!(PolicyRule::DenyAll.validate().is_ok());
    }

    #[test]
    fn validate_good_allow_is_ok() {
        assert!(PolicyRule::allow("view_file").validate().is_ok());
    }

    #[test]
    fn push_rejects_invalid_rule() {
        let mut set = PolicySet::new();
        assert!(set.push(PolicyRule::Allow(String::new())).is_err());
        assert!(set.is_empty(), "invalid rule should not be added");
    }

    #[test]
    fn push_accepts_valid_rule() {
        let mut set = PolicySet::new();
        assert!(set.push(PolicyRule::allow("view_file")).is_ok());
        assert_eq!(set.len(), 1);
    }

    // ── with_rule() builder tests ────────────────────────────────────

    #[test]
    fn with_rule_builder_chains() {
        let set = PolicySet::new()
            .with_rule(PolicyRule::allow("view_file"))
            .unwrap()
            .with_rule(PolicyRule::DenyAll)
            .unwrap();
        assert_eq!(set.len(), 2);
        assert!(set.evaluate("view_file").is_allowed());
        assert!(set.evaluate("unknown").is_denied());
    }

    #[test]
    fn with_rule_rejects_invalid() {
        let result = PolicySet::new().with_rule(PolicyRule::Allow(String::new()));
        assert!(result.is_err());
    }

    // ── PolicySet::validated_from tests ───────────────────────────────

    #[test]
    fn validated_from_vec_valid_rules() {
        let rules = vec![
            PolicyRule::allow("view_file"),
            PolicyRule::deny("run_command"),
            PolicyRule::DenyAll,
        ];
        let set = PolicySet::validated_from(rules).expect("valid rules");
        assert_eq!(set.len(), 3);
        assert!(set.evaluate("view_file").is_allowed());
        assert!(set.evaluate("run_command").is_denied());
    }

    #[test]
    fn validated_from_vec_empty_is_valid() {
        let set = PolicySet::validated_from(vec![]).expect("empty vec is valid");
        assert!(set.is_empty());
    }

    #[test]
    fn validated_from_vec_rejects_invalid_rule() {
        let rules = vec![
            PolicyRule::allow("view_file"),
            PolicyRule::Allow(String::new()), // invalid: empty name
            PolicyRule::DenyAll,
        ];
        let result = PolicySet::validated_from(rules);
        assert!(result.is_err(), "should reject empty tool name");
    }

    #[test]
    fn validated_from_vec_rejects_empty_ask_user_handler() {
        let rules = vec![PolicyRule::AskUser {
            tool: "run_command".into(),
            handler_id: "  ".into(), // invalid: whitespace-only
        }];
        let result = PolicySet::validated_from(rules);
        assert!(result.is_err(), "should reject empty handler_id");
    }

    #[test]
    fn validated_from_vec_rejects_empty_workspace_only() {
        let rules = vec![PolicyRule::WorkspaceOnly(vec![])];
        let result = PolicySet::validated_from(rules);
        assert!(result.is_err(), "should reject empty workspace list");
    }

    // ── From<Vec<PolicyRule>> validation regression tests ─────────────

    #[test]
    #[should_panic(expected = "invalid rules")]
    fn from_vec_panics_on_empty_tool_name() {
        // From<Vec<PolicyRule>> must now validate — empty tool name panics.
        let _set = PolicySet::from(vec![PolicyRule::Allow(String::new())]);
    }

    #[test]
    #[should_panic(expected = "invalid rules")]
    fn from_vec_panics_on_empty_handler_id() {
        let _set = PolicySet::from(vec![PolicyRule::AskUser {
            tool: "run_command".into(),
            handler_id: "  ".into(),
        }]);
    }

    #[test]
    #[should_panic(expected = "invalid rules")]
    fn from_vec_panics_on_empty_workspace_only() {
        let _set = PolicySet::from(vec![PolicyRule::WorkspaceOnly(vec![])]);
    }

    #[test]
    fn from_vec_accepts_valid_rules() {
        // Valid rules should not panic.
        let set = PolicySet::from(vec![
            PolicyRule::allow("view_file"),
            PolicyRule::deny("run_command"),
            PolicyRule::DenyAll,
        ]);
        assert_eq!(set.len(), 3);
    }

    // ── validated_from additional coverage ────────────────────────────

    #[test]
    fn validated_from_valid_rules_via_vec() {
        let rules = vec![
            PolicyRule::allow("view_file"),
            PolicyRule::deny("run_command"),
            PolicyRule::DenyAll,
        ];
        let set = PolicySet::validated_from(rules).expect("valid rules");
        assert_eq!(set.len(), 3);
        assert!(set.evaluate("view_file").is_allowed());
    }

    #[test]
    fn validated_from_invalid_rule_via_vec_is_err() {
        let rules = vec![PolicyRule::Allow(String::new())];
        assert!(PolicySet::validated_from(rules).is_err());
    }

    #[test]
    fn validated_from_single_allow_all() {
        let set = PolicySet::validated_from(vec![PolicyRule::AllowAll]).expect("valid rules");
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn validated_from_empty_workspace_only_is_err() {
        assert!(PolicySet::validated_from(vec![PolicyRule::WorkspaceOnly(vec![])]).is_err());
    }
}

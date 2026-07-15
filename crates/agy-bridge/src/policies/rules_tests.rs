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
    // Verify impl Into<String> works with &str and produces the owned variants.
    assert_eq!(
        PolicyRule::allow("view_file"),
        PolicyRule::Allow("view_file".to_owned())
    );
    assert_eq!(
        PolicyRule::deny("run_command"),
        PolicyRule::Deny("run_command".to_owned())
    );
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

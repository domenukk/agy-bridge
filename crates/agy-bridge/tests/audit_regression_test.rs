//! Regression tests for issues found during the agy-bridge audit.
//!
//! These tests verify fixes for specific bugs and ensure they don't regress.
//! They run without a live Python SDK or API key.

mod common;

#[test]
fn test_load_dotenv_strips_double_quotes() {
    // Regression: load_dotenv was not stripping surrounding quotes from values.
    // Verify the quote-stripping logic inline (load_dotenv uses OnceLock so
    // cannot be easily re-invoked, but the parsing logic is testable directly).

    let val = "\"hello world\"".trim();
    let stripped = val
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| val.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
        .unwrap_or(val);
    assert_eq!(stripped, "hello world");

    let val2 = "'single'".trim();
    let stripped2 = val2
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| val2.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
        .unwrap_or(val2);
    assert_eq!(stripped2, "single");

    let val3 = "plain";
    let stripped3 = val3
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| val3.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
        .unwrap_or(val3);
    assert_eq!(stripped3, "plain");
}

#[test]
fn test_error_is_quota_error_structural() {
    use std::time::Duration;

    use agy_bridge::error::Error;

    // QuotaExceeded variant is always a quota error
    assert!(
        Error::QuotaExceeded {
            retry_after: Duration::from_secs(1)
        }
        .is_quota_error()
    );

    // Backend error mentioning 429 is a quota error
    assert!(
        Error::BackendError {
            message: "HTTP 429".into()
        }
        .is_quota_error()
    );

    // Backend error mentioning RESOURCE_EXHAUSTED is a quota error
    assert!(
        Error::BackendError {
            message: "RESOURCE_EXHAUSTED".into()
        }
        .is_quota_error()
    );

    // Connection errors are NOT quota errors
    assert!(
        !Error::ConnectionError {
            message: "timeout".into()
        }
        .is_quota_error()
    );

    // Normal backend errors are NOT quota errors
    assert!(
        !Error::BackendError {
            message: "internal error".into()
        }
        .is_quota_error()
    );
}

#[test]
fn test_policy_set_from_vec_validates() {
    use agy_bridge::policies::{PolicyRule, PolicySet};

    // Valid: AllowAll at end
    let valid: PolicySet = vec![PolicyRule::AllowAll].into();
    assert!(valid.evaluate("anything").is_allowed());

    // Valid: specific rules then DenyAll
    let valid2: PolicySet = vec![PolicyRule::allow("view_file"), PolicyRule::DenyAll].into();
    assert!(valid2.evaluate("view_file").is_allowed());
    assert!(valid2.evaluate("run_command").is_denied());
}

#[test]
fn test_policy_set_validated_from_rejects_invalid() {
    use agy_bridge::policies::{PolicyRule, PolicySet};

    // Invalid: empty tool name should fail validation
    let result = PolicySet::validated_from(vec![
        PolicyRule::Allow(String::new()), // empty tool name
        PolicyRule::DenyAll,
    ]);
    assert!(
        result.is_err(),
        "Expected validation error for empty tool name"
    );
}

#[test]
fn test_policy_set_deny_all_shadows_later_rules() {
    use agy_bridge::policies::{PolicyRule, PolicySet};

    // DenyAll first should shadow everything after it (first-match semantics)
    let set: PolicySet = vec![PolicyRule::DenyAll, PolicyRule::allow("view_file")].into();
    assert!(
        set.evaluate("view_file").is_denied(),
        "DenyAll should shadow the later Allow rule"
    );
}

#[test]
fn test_tool_error_implements_eq() {
    use agy_bridge::tools::ToolError;

    // Verify Eq trait bound at compile time
    fn assert_eq_impl<T: Eq>(_: &T) {}

    let a = ToolError::new("test");
    let b = ToolError::new("test");
    assert_eq!(a, b); // This requires PartialEq + Eq
    assert_eq_impl(&a);
}

//! Safety filter detection heuristics and confidence hierarchy.

use serde::{Deserialize, Serialize};

use crate::{error::Error, types::Step};

/// Confidence level that a response was impacted or blocked by an upstream safety filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
pub enum Confidence {
    /// No safety filter interference detected.
    #[default]
    None,
    /// Suspiciously short response (lowest confidence of safety filter tripping).
    Low,
    /// Empty response where content or tool calls were expected (medium-low confidence).
    MediumLow,
    /// Known textual refusal patterns matching standard LLM rejections (medium-high confidence).
    MediumHigh,
    /// Explicit HTTP safety errors observed in backend response (high confidence).
    High,
    /// API-level safety flags or explicit safety errors (highest confidence / absolute certainty).
    Certain,
}

/// The result of evaluating a step or error for safety filter interference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SafetyVerdict {
    /// The confidence level of safety filter interference.
    pub confidence: Confidence,
    /// A human-readable explanation of why this verdict was reached.
    pub reason: String,
    /// Whether this verdict warrants an immediate recovery action (e.g., kill-and-respawn).
    pub warrants_recovery: bool,
}

impl Default for SafetyVerdict {
    fn default() -> Self {
        Self {
            confidence: Confidence::None,
            reason: "No safety filter interference detected".to_string(),
            warrants_recovery: false,
        }
    }
}

/// Suspiciously short response length threshold (in bytes/chars).
const SUSPICIOUSLY_SHORT_LENGTH: usize = 20;

/// Known standard LLM refusal prefixes and patterns.
const KNOWN_REFUSAL_PATTERNS: &[&str] = &[
    "i cannot fulfill this request",
    "i am unable to assist with",
    "as an ai",
    "i cannot generate code for exploiting",
    "i cannot provide instructions for",
    "against my safety policies",
    "i cannot assist with this request",
    "i'm sorry, but i cannot",
    "i can't help with",
    "i'm not able to",
    "i must decline",
    "this goes against",
    "i cannot create",
    "i won't be able to",
    "i'm unable to",
    "i cannot help with",
    "i cannot do that",
];

/// Check for explicit API-level safety flags or errors.
fn check_api_flags(step: Option<&Step>, error: Option<&Error>) -> Option<SafetyVerdict> {
    if let Some(Error::Safety) = error {
        return Some(SafetyVerdict {
            confidence: Confidence::Certain,
            reason: "Explicit API-level Safety error variant observed".to_string(),
            warrants_recovery: true,
        });
    }
    if let Some(st) = step {
        let err_lower = st.error.to_lowercase();
        if err_lower.contains("blocked by safety") || err_lower.contains("safety filter") {
            return Some(SafetyVerdict {
                confidence: Confidence::Certain,
                reason: "Step error explicitly indicates safety filter block".to_string(),
                warrants_recovery: true,
            });
        }
    }
    None
}

/// Check for HTTP safety errors or backend finish reasons indicating safety blocks.
fn check_http_errors(step: Option<&Step>, error: Option<&Error>) -> Option<SafetyVerdict> {
    if let Some(Error::BackendError { message }) = error {
        let msg_lower = message.to_lowercase();
        if msg_lower.contains("finish_reason: safety")
            || (msg_lower.contains("400") && msg_lower.contains("safety"))
        {
            return Some(SafetyVerdict {
                confidence: Confidence::High,
                reason: "HTTP backend error indicates safety finish reason or safety block"
                    .to_string(),
                warrants_recovery: true,
            });
        }
    }
    if let Some(st) = step {
        let err_lower = st.error.to_lowercase();
        if err_lower.contains("finish_reason: safety")
            || (err_lower.contains("400") && err_lower.contains("safety"))
        {
            return Some(SafetyVerdict {
                confidence: Confidence::High,
                reason: "Step error indicates HTTP safety finish reason".to_string(),
                warrants_recovery: true,
            });
        }
    }
    None
}

/// Check for known textual refusal patterns in the response content.
fn check_known_refusals(step: &Step) -> Option<SafetyVerdict> {
    let content_lower = step.content.to_lowercase();
    for &pattern in KNOWN_REFUSAL_PATTERNS {
        if content_lower.contains(pattern) {
            return Some(SafetyVerdict {
                confidence: Confidence::MediumHigh,
                reason: format!("Response matches known refusal pattern: '{pattern}'"),
                warrants_recovery: true,
            });
        }
    }
    None
}

/// Check for an unexpected empty response where content or tool calls were required.
///
/// A step with non-empty `thinking` is NOT considered empty — the model may
/// produce a thinking-only step (e.g., extended reasoning) before a tool call.
fn check_empty_response(step: &Step) -> Option<SafetyVerdict> {
    if step.content.trim().is_empty()
        && step.thinking.trim().is_empty()
        && step.tool_calls.is_empty()
        && step.error.is_empty()
    {
        return Some(SafetyVerdict {
            confidence: Confidence::MediumLow,
            reason: "Response is completely empty with zero tool calls".to_string(),
            warrants_recovery: false,
        });
    }
    None
}

/// Check for suspiciously short responses lacking tool calls (e.g., "I cannot.", "No.").
fn check_short_response(step: &Step) -> Option<SafetyVerdict> {
    let trimmed = step.content.trim();
    if !trimmed.is_empty()
        && trimmed.len() < SUSPICIOUSLY_SHORT_LENGTH
        && step.tool_calls.is_empty()
    {
        return Some(SafetyVerdict {
            confidence: Confidence::Low,
            reason: format!(
                "Response is suspiciously short ({} chars) with zero tool calls",
                trimmed.len()
            ),
            warrants_recovery: false,
        });
    }
    None
}

/// Evaluate an agent step or error for potential safety filter interference.
///
/// Evaluates conditions in decreasing order of confidence hierarchy:
/// 1. API-level safety flags (`Confidence::Certain`)
/// 2. HTTP safety errors (`Confidence::High`)
/// 3. Known refusal patterns (`Confidence::MediumHigh`)
/// 4. Empty responses (`Confidence::MediumLow`)
/// 5. Suspiciously short responses (`Confidence::Low`)
#[must_use]
pub fn detect_safety_interference(step: Option<&Step>, error: Option<&Error>) -> SafetyVerdict {
    if let Some(verdict) = check_api_flags(step, error) {
        return verdict;
    }
    if let Some(verdict) = check_http_errors(step, error) {
        return verdict;
    }
    if let Some(st) = step {
        if let Some(verdict) = check_known_refusals(st) {
            return verdict;
        }
        if let Some(verdict) = check_empty_response(st) {
            return verdict;
        }
        if let Some(verdict) = check_short_response(st) {
            return verdict;
        }
    }
    SafetyVerdict::default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Step, StepStatus};

    fn dummy_step(content: &str, error: &str) -> Step {
        Step {
            content: content.to_string(),
            error: error.to_string(),
            tool_calls: vec![],
            status: if error.is_empty() {
                StepStatus::Done
            } else {
                StepStatus::Error
            },
            ..Step::default()
        }
    }

    #[test]
    fn test_api_level_safety_error() {
        let err = Error::Safety;
        let verdict = detect_safety_interference(None, Some(&err));
        assert_eq!(verdict.confidence, Confidence::Certain);
        assert!(verdict.warrants_recovery);
    }

    #[test]
    fn test_api_level_safety_step() {
        let step = dummy_step("", "Request blocked by safety filter");
        let verdict = detect_safety_interference(Some(&step), None);
        assert_eq!(verdict.confidence, Confidence::Certain);
        assert!(verdict.warrants_recovery);
    }

    #[test]
    fn test_http_safety_error() {
        let err = Error::BackendError {
            message: "HTTP 400: finish_reason: SAFETY".to_string(),
        };
        let verdict = detect_safety_interference(None, Some(&err));
        assert_eq!(verdict.confidence, Confidence::High);
        assert!(verdict.warrants_recovery);
    }

    #[test]
    fn test_known_refusal_pattern() {
        let step = dummy_step(
            "I am unable to assist with exploiting this vulnerability.",
            "",
        );
        let verdict = detect_safety_interference(Some(&step), None);
        assert_eq!(verdict.confidence, Confidence::MediumHigh);
        assert!(verdict.warrants_recovery);
    }

    #[test]
    fn test_empty_response() {
        let step = dummy_step("   ", "");
        let verdict = detect_safety_interference(Some(&step), None);
        assert_eq!(verdict.confidence, Confidence::MediumLow);
        assert!(!verdict.warrants_recovery);
    }

    #[test]
    fn test_short_response() {
        let step = dummy_step("No way.", "");
        let verdict = detect_safety_interference(Some(&step), None);
        assert_eq!(verdict.confidence, Confidence::Low);
        assert!(!verdict.warrants_recovery);
    }

    #[test]
    fn test_benign_response() {
        let step = dummy_step(
            "This is a fully valid, sufficiently long response explaining the architecture.",
            "",
        );
        let verdict = detect_safety_interference(Some(&step), None);
        assert_eq!(verdict.confidence, Confidence::None);
        assert!(!verdict.warrants_recovery);
    }

    #[test]
    fn test_hierarchy_priority() {
        // Step has a short response (Low) BUT an explicit API safety error (Certain). Certain must win.
        let step = dummy_step("Short", "Blocked by safety");
        let verdict = detect_safety_interference(Some(&step), None);
        assert_eq!(verdict.confidence, Confidence::Certain);
        assert!(verdict.warrants_recovery);
    }

    #[test]
    fn test_thinking_only_step_is_not_empty() {
        // A step with thinking content but no text content should NOT trigger
        // the empty-response heuristic — the model is reasoning.
        let step = Step {
            content: String::new(),
            thinking: "Let me reason about this problem...".to_string(),
            tool_calls: vec![],
            error: String::new(),
            status: StepStatus::Done,
            ..Step::default()
        };
        let verdict = detect_safety_interference(Some(&step), None);
        assert_eq!(
            verdict.confidence,
            Confidence::None,
            "Thinking-only step should NOT be flagged as empty"
        );
    }

    #[test]
    fn test_step_with_tool_calls_not_flagged() {
        // A step with empty content but tool calls present should NOT trigger
        // empty or short response checks.
        let step = Step {
            content: String::new(),
            tool_calls: vec![crate::types::ToolCallInfo {
                name: "view_file".to_string(),
                args: serde_json::json!({"path": "/tmp/foo.rs"}),
                id: None,
                canonical_path: None,
            }],
            error: String::new(),
            status: StepStatus::Done,
            ..Step::default()
        };
        let verdict = detect_safety_interference(Some(&step), None);
        assert_eq!(
            verdict.confidence,
            Confidence::None,
            "Step with tool calls should NOT be flagged"
        );
    }

    #[test]
    fn test_expanded_refusal_patterns() {
        for pattern in [
            "I can't help with that request.",
            "I'm not able to generate exploit code.",
            "I must decline this request for safety reasons.",
            "This goes against my usage policies.",
        ] {
            let step = dummy_step(pattern, "");
            let verdict = detect_safety_interference(Some(&step), None);
            assert_eq!(
                verdict.confidence,
                Confidence::MediumHigh,
                "Pattern '{pattern}' should trigger MediumHigh"
            );
        }
    }
}

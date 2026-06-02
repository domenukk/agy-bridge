//! Interactive question/answer types for agent-user interactions.

use serde::{Deserialize, Serialize};

use super::types::HookResult;

// ── Interactive Q&A types (Python SDK parity) ───────────────────────────────

/// A single selectable option in an interactive question prompt.
///
/// Maps to the Python SDK's `AskQuestionOption`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskQuestionOption {
    /// Human-readable label displayed to the user.
    pub label: String,
    /// Machine-readable value returned when this option is selected.
    pub value: String,
}

/// A question with its list of selectable options.
///
/// Maps to the Python SDK's `AskQuestionEntry`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskQuestionEntry {
    /// The question text to present to the user.
    pub question: String,
    /// Available answer options.
    pub options: Vec<AskQuestionOption>,
}

/// Full specification for an interactive ask-user interaction.
///
/// Contains one or more questions to present to the user in sequence.
/// Maps to the Python SDK's `AskQuestionInteractionSpec`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskQuestionInteractionSpec {
    /// The questions to present to the user.
    pub entries: Vec<AskQuestionEntry>,
}

/// The user's response to an interactive question prompt.
///
/// Maps to the Python SDK's `QuestionResponse`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionResponse {
    /// The selected answer values, one per question entry.
    pub answers: Vec<String>,
}

/// Combined hook result and optional question response.
///
/// Returned by hooks that involve interactive Q&A. The `hook_result`
/// determines whether the agent should proceed, and `response` carries
/// the user's answers (if any).
///
/// Maps to the Python SDK's `QuestionHookResult`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionHookResult {
    /// The hook decision (allow/deny).
    pub hook_result: HookResult,
    /// The user's answers, if a question was asked and answered.
    pub response: Option<QuestionResponse>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ask_question_option_serde_roundtrip() {
        let opt = AskQuestionOption {
            label: "Yes".into(),
            value: "y".into(),
        };
        let json = serde_json::to_string(&opt).unwrap();
        let parsed: AskQuestionOption = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, opt);
    }

    #[test]
    fn ask_question_entry_serde_roundtrip() {
        let entry = AskQuestionEntry {
            question: "Continue?".into(),
            options: vec![
                AskQuestionOption {
                    label: "Yes".into(),
                    value: "y".into(),
                },
                AskQuestionOption {
                    label: "No".into(),
                    value: "n".into(),
                },
            ],
        };
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: AskQuestionEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, entry);
    }

    #[test]
    fn ask_question_interaction_spec_serde_roundtrip() {
        let spec = AskQuestionInteractionSpec {
            entries: vec![
                AskQuestionEntry {
                    question: "Q1?".into(),
                    options: vec![AskQuestionOption {
                        label: "A".into(),
                        value: "a".into(),
                    }],
                },
                AskQuestionEntry {
                    question: "Q2?".into(),
                    options: vec![],
                },
            ],
        };
        let json = serde_json::to_string(&spec).unwrap();
        let parsed: AskQuestionInteractionSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, spec);
    }

    #[test]
    fn question_response_serde_roundtrip() {
        let resp = QuestionResponse {
            answers: vec!["yes".into(), "fast".into()],
        };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: QuestionResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, resp);
    }

    #[test]
    fn question_hook_result_with_response() {
        let result = QuestionHookResult {
            hook_result: HookResult::allow_with_message("confirmed"),
            response: Some(QuestionResponse {
                answers: vec!["answer1".into()],
            }),
        };
        let json = serde_json::to_string(&result).unwrap();
        let parsed: QuestionHookResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.hook_result, result.hook_result);
        assert!(parsed.response.is_some());
    }

    #[test]
    fn question_hook_result_without_response() {
        let result = QuestionHookResult {
            hook_result: HookResult::deny("cancelled"),
            response: None,
        };
        let json = serde_json::to_string(&result).unwrap();
        let parsed: QuestionHookResult = serde_json::from_str(&json).unwrap();
        assert!(!parsed.hook_result.allow);
        assert!(parsed.response.is_none());
    }
}
